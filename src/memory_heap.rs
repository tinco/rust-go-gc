use super::memory_central::MemoryCentral;
use super::memory_span;
use super::memory_span::*;
use super::memory_span_list::*;
use super::memory_span_treap::*;
use super::size_classes::*;
use super::static_vec::*;
use super::sweep_buffer::*;
use crate::memory_allocator::*;
use array_init::array_init;
use cache_line_size::*;
use std::ops::{Deref, DerefMut};
use std::ptr::{NonNull, Unique};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Mutex, MutexGuard};
use std::time::Instant;

pub const MAX_MEMORY_HEAP_LIST: usize = 1 << (20 - PAGE_SHIFT); // Maximum page length for fixed-size list in MHeap.;

// ProtectedMemoryHeap contains all the fields of MemoryHeap that are protected by the mutex.
pub struct ProtectedMemoryHeap {
    pub free: MemorySpanTreap, // free and non-scavenged spans
    pub scav: MemorySpanTreap, // free and scavenged spans

    // free      [_MaxMHeapList]mSpanList // free lists of given length up to _MaxMHeapList
    // freelarge mTreap                   // free treap of length >= _MaxMHeapList
    pub busy: [MemorySpanList; MAX_MEMORY_HEAP_LIST], // busy lists of large spans of given length
    pub busy_large: MemorySpanList, // busy lists of large spans length >= _MaxMHeapList

    // allspans is a slice of all mspans ever created. Each mspan
    // appears exactly once.
    //
    // The memory for allspans is manually managed and can be
    // reallocated and move as the heap grows.
    //
    // In general, allspans is protected by mheap_.lock, which
    // prevents concurrent access as well as freeing the backing
    // store. Accesses during STW might not hold the lock, but
    // must ensure that allocation cannot happen around the
    // access (since that may free the backing store).
    // allspans []*mspan // all spans out there

    // spans is a lookup table to map virtual address page IDs to *mspan.
    // For allocated spans, their pages map to the span itself.
    // For free spans, only the lowest and highest pages map to the span itself.
    // Internal pages map to an arbitrary span.
    // For pages that have never been allocated, spans entries are nil.
    //
    // This is backed by a reserved region of the address space so
    // it can grow without moving. The memory up to len(spans) is
    // mapped. cap(spans) indicates the total reserved memory.
    pub spans: StaticVec<MemorySpan>,
    //
    pub pages_in_use: u64, // pages of spans in stats _MSpanInUse; R/W with mheap.lock

    // arenas is the heap arena map. It points to the metadata for
    // the heap for every arena frame of the entire usable virtual
    // address space.
    //
    // Use arenaIndex to compute indexes into this array.
    //
    // For regions of the address space that are not backed by the
    // Go heap, the arena map contains nil.
    //
    // Modifications are protected by mheap_.lock. Reads can be
    // performed without locking; however, a given entry can
    // transition from nil to non-nil at any time when the lock
    // isn't held. (Entries never transitions back to nil.)
    //
    // In general, this is a two-level mapping consisting of an L1
    // map and possibly many L2 maps. This saves space when there
    // are a huge number of arena frames. However, on many
    // platforms (even 64-bit), arenaL1Bits is 0, making this
    // effectively a single-level map. In this case, arenas[0]
    // will never be nil.
    pub arenas: [*mut [*mut HeapArena; 1 << ARENA_LEVEL_2_BITS]; 1 << ARENA_LEVEL_1_BITS],
}

// A heapArena stores metadata for a heap arena. heapArenas are stored
// outside of the Go heap and accessed via the mheap_.arenas index.
//
// This gets allocated directly from the OS, so ideally it should be a
// multiple of the system page size. For example, avoid adding small
// fields.
//
//go:notinheap
pub struct HeapArena {
    // bitmap stores the pointer/scalar bitmap for the words in
    // this arena. See mbitmap.go for a description. Use the
    // heapBits type to access this.
    // bitmap [heapArenaBitmapBytes]byte

    // spans maps from virtual address page ID within this arena to *mspan.
    // For allocated spans, their pages map to the span itself.
    // For free spans, only the lowest and highest pages map to the span itself.
    // Internal pages map to an arbitrary span.
    // For pages that have never been allocated, spans entries are nil.
    //
    // Modifications are protected by mheap.lock. Reads can be
    // performed without locking, but ONLY from indexes that are
    // known to contain in-use or stack spans. This means there
    // must not be a safe-point between establishing that an
    // address is live and looking it up in the spans array.
    pub spans: [MemorySpan; PAGES_PER_ARENA],

    // pageInUse is a bitmap that indicates which spans are in
    // state mSpanInUse. This bitmap is indexed by page number,
    // but only the bit corresponding to the first page in each
    // span is used.
    //
    // Writes are protected by mheap_.lock.
    pub page_in_use: [u8; PAGES_PER_ARENA / 8],

    // pageMarks is a bitmap that indicates which spans have any
    // marked objects on them. Like pageInUse, only the bit
    // corresponding to the first page in each span is used.
    //
    // Writes are done atomically during marking. Reads are
    // non-atomic and lock-free since they only occur during
    // sweeping (and hence never race with writes).
    //
    // This is used to quickly find whole spans that can be freed.
    //
    // TODO(austin): It would be nice if this was uint64 for
    // faster scanning, but we don't have 64-bit atomic bit
    // operations.
    pub page_marks: [u8; PAGES_PER_ARENA / 8],
}

// Main malloc heap.
// The heap itself is the "free[]" and "large" arrays,
// but all the other global data is here too.
//
pub struct MemoryHeap {
    // Fields in protected *might* need acquisition of the lock
    protected: ProtectedMemoryHeap,
    lock: Mutex<Unique<LockedMemoryHeap>>,
    // TODO these should all be u32!
    pub sweep_generation: AtomicUsize, // sweep generation, see comment in mspan, is increased by 2 after every GC
    pub sweep_done: AtomicBool,        // all spans are swept
    pub sweepers: AtomicUsize,         // number of active sweepone calls
    // _ uint32 // align uint64 fields on 32-bit for atomics

    // sweep_buffers contains two mspan stacks: one of swept in-use
    // spans, and one of unswept in-use spans. These two trade
    // roles on each GC cycle. Since the sweepgen increases by 2
    // on each cycle, this means the swept spans are in
    // sweep_buffers[sweepgen/2%2] and the unswept spans are in
    // sweep_buffers[1-sweepgen/2%2]. Sweeping pops spans from the
    // unswept stack and pushes spans that are still in-use on the
    // swept stack. Likewise, allocating an in-use span pushes it
    // on the swept stack.
    sweep_buffers: [SweepBuffer; 2],

    // // Proportional sweep
    // //
    // // These parameters represent a linear function from heap_live
    // // to page sweep count. The proportional sweep system works to
    // // stay in the black by keeping the current page sweep count
    // // above this line at the current heap_live.
    // //
    // // The line has slope sweepPagesPerByte and passes through a
    // // basis point at (sweepHeapLiveBasis, pagesSweptBasis). At
    // // any given time, the system is at (memstats.heap_live,
    // // pagesSwept) in this space.
    // //
    // // It's important that the line pass through a point we
    // // control rather than simply starting at a (0,0) origin
    // // because that lets us adjust sweep pacing at any time while
    // // accounting for current progress. If we could only adjust
    // // the slope, it would create a discontinuity in debt if any
    // // progress has already been made.
    pub pages_swept: AtomicUsize, // pages swept this cycle; updated atomically

    // pagesSweptBasis    uint64  // pagesSwept to use as the origin of the sweep ratio; updated atomically
    // sweepHeapLiveBasis uint64  // value of heap_live to use as the origin of sweep ratio; written with lock, read without
    // sweepPagesPerByte  float64 // proportional sweep ratio; written with lock, read without
    // // TODO(austin): pagesInUse should be a uintptr, but the 386
    // // compiler can't 8-byte align fields.
    //
    // // Malloc stats.
    // largealloc  uint64                  // bytes allocated for large objects
    // nlargealloc uint64                  // number of large object allocations
    // largefree   uint64                  // bytes freed for large objects (>maxsmallsize)
    // nlargefree  uint64                  // number of frees for large objects (>maxsmallsize)
    // nsmallfree  [_NumSizeClasses]uint64 // number of frees for small objects (<=maxsmallsize)
    //
    // // range of addresses we might see in the heap
    // bitmap        uintptr // Points to one byte past the end of the bitmap
    // bitmap_mapped uintptr
    //
    // // The arena_* fields indicate the addresses of the Go heap.
    // //
    // // The maximum range of the Go heap is
    // // [arena_start, arena_start+_MaxMem+1).
    // //
    // // The range of the current Go heap is
    // // [arena_start, arena_used). Parts of this range may not be
    // // mapped, but the metadata structures are always mapped for
    // // the full range.
    pub arena_start: AtomicUsize,
    pub arena_used: AtomicUsize, // Set with setArenaUsed.
    //
    // // The heap is grown using a linear allocator that allocates
    // // from the block [arena_alloc, arena_end). arena_alloc is
    // // often, but *not always* equal to arena_used.
    // arena_alloc uintptr
    // arena_end   uintptr
    //
    // // arena_reserved indicates that the memory [arena_alloc,
    // // arena_end) is reserved (e.g., mapped PROT_NONE). If this is
    // // false, we have to be careful not to clobber existing
    // // mappings here. If this is true, then we own the mapping
    // // here and *must* clobber it to use it.
    // arena_reserved bool
    //
    // _ uint32 // ensure 64-bit alignment
    //
    // central free lists for small size classes.
    pub central: [CacheAligned<MemoryCentral>; NUM_SPAN_CLASSES.0 as usize],
    //
    // spanalloc             fixalloc // allocator for span*
    // cachealloc            fixalloc // allocator for mcache*
    // treapalloc            fixalloc // allocator for treapNodes* used by large objects
    // specialfinalizeralloc fixalloc // allocator for specialfinalizer*
    // specialprofilealloc   fixalloc // allocator for specialprofile*
    // speciallock           mutex    // lock for special record allocators.
}

impl MemoryHeap {
    pub fn get_pushable_sweep_buffer(&self, generation: usize) -> &PushableSweepBuffer {
        unsafe {
            std::mem::transmute::<&SweepBuffer, &PushableSweepBuffer>(
                &self.sweep_buffers[generation / 2 % 2],
            )
        }
    }

    pub fn get_popable_sweep_buffer(&self, generation: usize) -> &PopableSweepBuffer {
        unsafe {
            std::mem::transmute::<&SweepBuffer, &PopableSweepBuffer>(
                &self.sweep_buffers[1 - generation / 2 % 2],
            )
        }
    }

    pub fn lock(&mut self) -> LockedMemoryHeapGuard {
        LockedMemoryHeapGuard {
            0: self.lock.lock().expect("Could not get MemoryHeap lock"),
        }
    }

    // freeManual frees a manually-managed span returned by allocManual.
    // stat must be the same as the stat passed to the allocManual that
    // allocated s.
    //
    // This must only be called when gcphase == _GCoff. See mSpanState for
    // an explanation.
    //
    // freeManual must be called on the system stack to prevent stack
    // growth, just like allocManual.
    //
    //go:systemstack
    pub fn free_manual_span(&mut self, mut span: MemorySpan /*, stat *uint64 */) {
        unsafe { span.as_mut() }.need_zero = 1;
        // *stat -= uint64(s.npages << _PageShift)
        // memstats.heap_sys += uint64(s.npages << _PageShift)
        let mut memory_heap = self.lock();
        memory_heap.free_span(span, false, true, None);
    }

    // Free the span back into the heap.
    pub fn free_span(&mut self, span: MemorySpan, account: i32) {
        let mut memory_heap = self.lock();

        // 	memstats.heap_scan += uint64(mp.mcache.local_scan)
        // 	mp.mcache.local_scan = 0
        // 	memstats.tinyallocs += uint64(mp.mcache.local_tinyallocs)
        // 	mp.mcache.local_tinyallocs = 0
        // 	if msanenabled {
        // 		// Tell msan that this entire span is no longer in use.
        // 		base := unsafe.Pointer(s.base())
        // 		bytes := s.npages << _PageShift
        // 		msanfree(base, bytes)
        // 	}
        // 	if acct != 0 {
        // 		memstats.heap_objects--
        // 	}
        // 	if gcBlackenEnabled != 0 {
        // 		// heap_scan changed.
        // 		gcController.revise()
        // 	}

        memory_heap.free_span(span, true, true, None);
    }

    pub fn new() -> Box<Self> {
        let memory_central: [CacheAligned<MemoryCentral>; NUM_SPAN_CLASSES.0 as usize] =
            array_init(|i| CacheAligned {
                0: MemoryCentral::new(),
            });
        let busy_span_lists: [MemorySpanList; MAX_MEMORY_HEAP_LIST] =
            array_init(|i| MemorySpanList::new());

        let arenas: [*mut _; 1] = array_init(|i| std::ptr::null_mut());

        let memory_heap = MemoryHeap {
            lock: Mutex::new(Unique::empty()),
            protected: ProtectedMemoryHeap {
                pages_in_use: 0,
                busy: busy_span_lists,
                busy_large: MemorySpanList::new(),
                spans: StaticVec::new(0).expect("Could not allocate MemoryHeap spans"),
                arenas: arenas,
                free: MemorySpanTreap::new(),
                scav: MemorySpanTreap::new(),
            },
            arena_start: AtomicUsize::new(0),
            arena_used: AtomicUsize::new(0),
            sweep_done: AtomicBool::new(false),
            sweepers: AtomicUsize::new(0),
            sweep_generation: AtomicUsize::new(0),
            sweep_buffers: [SweepBuffer::new(), SweepBuffer::new()],
            pages_swept: AtomicUsize::new(0),
            central: memory_central,
        };

        let mut boxed = Box::new(memory_heap);

        let memory_heap_ptr: Unique<MemoryHeap> = unsafe { Unique::new_unchecked(boxed.as_mut()) };
        let memory_heap_locked_ptr = unsafe {
            std::mem::transmute::<Unique<MemoryHeap>, Unique<LockedMemoryHeap>>(memory_heap_ptr)
        };
        let lock = Mutex::new(memory_heap_locked_ptr);

        boxed.lock = lock;
        boxed
    }
}

pub struct LockedMemoryHeap(pub MemoryHeap);

impl LockedMemoryHeap {
    // span must be on a busy list (h.busy or h.busylarge) or unlinked.
    pub fn free_span(
        &mut self,
        mut unique_span: MemorySpan,
        account_in_use: bool,
        account_idle: bool,
        unused_since: Option<Instant>,
    ) {
        let mut span = unsafe { unique_span.as_mut() };

        match span.state {
            memory_span::State::Manual => {
                if span.allocations_count != 0 {
                    panic!("MHeap_FreeSpanLocked - invalid stack free")
                }
            }
            memory_span::State::InUse => {
                if span.allocations_count != 0
                    || span.sweep_generation.load(Ordering::Relaxed)
                        != self.0.sweep_generation.load(Ordering::Relaxed)
                {
                    // print("MHeap_FreeSpanLocked - span ", s, " ptr ", hex(s.base()), " allocCount ", s.allocCount, " sweepgen ", s.sweepgen, "/", h.sweepgen, "\n")
                    panic!("MHeap_FreeSpanLocked - invalid free")
                }
                self.0.protected.pages_in_use -= span.number_of_pages as u64

                // Clear in-use bit in arena page bitmap.
                // TODO
                // arena, pageIdx, pageMask := pageIndexOf(s.base())
                // arena.pageInUse[pageIdx] &^= pageMask
            }
            _ => panic!("MHeap_FreeSpanLocked - invalid span state"),
        }

        // 	if acctinuse {
        // 		memstats.heap_inuse -= uint64(s.npages << _PageShift)
        // 	}
        // 	if acctidle {
        // 		memstats.heap_idle += uint64(s.npages << _PageShift)
        // 	}

        span.state = memory_span::State::Free;

        // Stamp newly unused spans. The scavenger will use that
        // info to potentially give back some pages to the OS.
        span.unused_since = match unused_since {
            Some(instant) => instant,
            None => Instant::now(),
        };

        // Coalesce span with neighbors.
        self.coalesce(span)

        // Insert s into the appropriate treap.
        // if span.scavenged {
        //     self.scav.insert(s)
        // } else {
        //     self.free.insert(s)
        // }
    }

    pub fn coalesce(&mut self, span: &mut MemorySpanData) {
        // We scavenge s at the end after coalescing if s or anything
        // it merged with is marked scavenged.
        let needs_scavenge = false;
        let mut prescavenged = span.released(); // number of bytes already scavenged.
        let span_non_null_ptr = unsafe { NonNull::new_unchecked(span as *mut MemorySpanData) };

        // merge is a helper which merges other into s, deletes references to other
        // in heap metadata, and then discards it. other must be adjacent to s.
        let merge = |mut other_unique: MemorySpan| {
            let other = unsafe { other_unique.as_mut() };
            // Adjust s via base and npages and also in heap metadata.
            span.number_of_pages += other.number_of_pages;
            span.need_zero |= other.need_zero;
            if other.start_address.as_ptr() < span.start_address.as_ptr() {
                span.start_address = other.start_address;
                self.set_span(span.base().as_ptr(), span_non_null_ptr)
            } else {
                let offset = span.number_of_pages * PAGE_SIZE - 1;
                let new_base = unsafe { span.base().as_ptr().add(offset) };
                self.set_span(new_base, span_non_null_ptr)
            }
            //
            // If before or s are scavenged, then we need to scavenge the final coalesced span.
            let needs_scavenge = needs_scavenge || other.scavenged || span.scavenged;
            prescavenged += other.released();

            // The size is potentially changing so the treap needs to delete adjacent nodes and
            // insert back as a combined node.
            if other.scavenged {
            	self.0.protected.scav.remove_span(other)
            } else {
            	self.0.protected.free.remove_span(other)
            }
            // 	other.state = mSpanDead
            // 	h.spanalloc.free(unsafe.Pointer(other))
        };
        //
        // // realign is a helper which shrinks other and grows s such that their
        // // boundary is on a physical page boundary.
        // realign := func(a, b, other *mspan) {
        // 	// Caller must ensure a.startAddr < b.startAddr and that either a or
        // 	// b is s. a and b must be adjacent. other is whichever of the two is
        // 	// not s.
        //
        // 	// If pageSize <= physPageSize then spans are always aligned
        // 	// to physical page boundaries, so just exit.
        // 	if pageSize <= physPageSize {
        // 		return
        // 	}
        // 	// Since we're resizing other, we must remove it from the treap.
        // 	if other.scavenged {
        // 		h.scav.removeSpan(other)
        // 	} else {
        // 		h.free.removeSpan(other)
        // 	}
        // 	// Round boundary to the nearest physical page size, toward the
        // 	// scavenged span.
        // 	boundary := b.startAddr
        // 	if a.scavenged {
        // 		boundary &^= (physPageSize - 1)
        // 	} else {
        // 		boundary = (boundary + physPageSize - 1) &^ (physPageSize - 1)
        // 	}
        // 	a.npages = (boundary - a.startAddr) / pageSize
        // 	b.npages = (b.startAddr + b.npages*pageSize - boundary) / pageSize
        // 	b.startAddr = boundary
        //
        // 	h.setSpan(boundary-1, a)
        // 	h.setSpan(boundary, b)
        //
        // 	// Re-insert other now that it has a new size.
        // 	if other.scavenged {
        // 		h.scav.insert(other)
        // 	} else {
        // 		h.free.insert(other)
        // 	}
        // }
        //
        // // Coalesce with earlier, later spans.
        // if before := spanOf(s.base() - 1); before != nil && before.state == mSpanFree {
        // 	if s.scavenged == before.scavenged {
        // 		merge(before)
        // 	} else {
        // 		realign(before, s, before)
        // 	}
        // }
        //
        // // Now check to see if next (greater addresses) span is free and can be coalesced.
        // if after := spanOf(s.base() + s.npages*pageSize); after != nil && after.state == mSpanFree {
        // 	if s.scavenged == after.scavenged {
        // 		merge(after)
        // 	} else {
        // 		realign(s, after, after)
        // 	}
        // }
        //
        // if needsScavenge {
        // 	// When coalescing spans, some physical pages which
        // 	// were not returned to the OS previously because
        // 	// they were only partially covered by the span suddenly
        // 	// become available for scavenging. We want to make sure
        // 	// those holes are filled in, and the span is properly
        // 	// scavenged. Rather than trying to detect those holes
        // 	// directly, we collect how many bytes were already
        // 	// scavenged above and subtract that from heap_released
        // 	// before re-scavenging the entire newly-coalesced span,
        // 	// which will implicitly bump up heap_released.
        // 	memstats.heap_released -= uint64(prescavenged)
        // 	s.scavenge()
        // }
    }

    // setSpan modifies the span map so spanOf(base) is s.
    fn set_span(&mut self, base: *mut u8, span: MemorySpan) {
        let arena_index = arena_index(base as usize);
        let span_index = (base as usize / PAGE_SIZE) % PAGES_PER_ARENA;
        let mut arena_1 = self.0.protected.arenas[arena_level_1(arena_index)];
        let mut arena_2 = unsafe { *arena_1 }[arena_level_2(arena_index)];
        unsafe { &mut *arena_2 }.spans[span_index] = span;
    }

    // TODO I think this got removed in the Go codebase
    // pub fn busy_list(&mut self, number_of_pages: usize) -> &mut MemorySpanList {
    //     let protected = &mut self.0.protected;
    //     if number_of_pages < protected.busy.len() {
    //         return &mut protected.busy[number_of_pages];
    //     }
    //     return &mut protected.busy_large;
    // }
}

// arenaIndex returns the index into mheap_.arenas of the arena
// containing metadata for p. This index combines of an index into the
// L1 map and an index into the L2 map and should be used as
// mheap_.arenas[ai.l1()][ai.l2()].
//
// If p is outside the range of valid heap addresses, either l1() or
// l2() will be out of bounds.
//
// It is nosplit because it's called by spanOf and several other
// nosplit functions.
//
//go:nosplit
pub fn arena_index(p: usize) -> ArenaIndex {
    ((p + ARENA_BASE_OFFSET) / HEAP_ARENA_BYTES) as ArenaIndex
}

type ArenaIndex = usize;

pub fn arena_level_1(i: ArenaIndex) -> usize {
    if ARENA_LEVEL_1_BITS == 0 {
        // Let the compiler optimize this away if there's no
        // L1 map.
        0
    } else {
        (i as usize) >> ARENA_LEVEL_1_SHIFT
    }
}

pub fn arena_level_2(i: ArenaIndex) -> usize {
    if ARENA_LEVEL_1_BITS == 0 {
        i as usize
    } else {
        (i as usize) & (1 << ARENA_LEVEL_2_BITS - 1)
    }
}

pub struct LockedMemoryHeapGuard<'a>(pub MutexGuard<'a, Unique<LockedMemoryHeap>>);
impl<'a> LockedMemoryHeapGuard<'a> {
    pub fn as_mut(&mut self) -> &mut LockedMemoryHeap {
        unsafe { self.0.as_mut() }
    }
}

impl<'a> Deref for LockedMemoryHeapGuard<'a> {
    type Target = LockedMemoryHeap;

    fn deref(&self) -> &LockedMemoryHeap {
        unsafe { self.0.as_ref() }
    }
}

impl<'a> DerefMut for LockedMemoryHeapGuard<'a> {
    fn deref_mut(&mut self) -> &mut LockedMemoryHeap {
        self.as_mut()
    }
}
