use super::memory_central::MemoryCentral;
use super::memory_span;
use super::memory_span::*;
use super::memory_span_list::*;
use super::size_classes::*;
use super::sweep_buffer::*;
use array_init::array_init;
use cache_line_size::*;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Mutex;

pub const MAX_MEMORY_HEAP_LIST: usize = 1 << (20 - PAGE_SHIFT); // Maximum page length for fixed-size list in MHeap.;

// ProtectedMemoryHeap contains all the fields of MemoryHeap that are protected by the mutex.
pub struct ProtectedMemoryHeap {
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
    // spans []*mspan
    //
    pub pages_in_use: u64, // pages of spans in stats _MSpanInUse; R/W with mheap.lock
}

impl ProtectedMemoryHeap {
    // span must be on a busy list (h.busy or h.busylarge) or unlinked.
    pub fn free_span(
        &mut self,
        memory_heap: &MemoryHeap,
        mut unique_span: MemorySpan,
        account_in_use: bool,
        account_idle: bool,
        unused_since: i64,
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
                        != memory_heap.sweep_generation.load(Ordering::Relaxed)
                {
                    // print("MHeap_FreeSpanLocked - span ", s, " ptr ", hex(s.base()), " allocCount ", s.allocCount, " sweepgen ", s.sweepgen, "/", h.sweepgen, "\n")
                    panic!("MHeap_FreeSpanLocked - invalid free")
                }
                self.pages_in_use -= span.number_of_pages as u64
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
        if span.is_in_list() {
            self.busy_list(span.number_of_pages).remove(unique_span)
        }

        // 	// Stamp newly unused spans. The scavenger will use that
        // 	// info to potentially give back some pages to the OS.
        // 	s.unusedsince = unusedsince
        // 	if unusedsince == 0 {
        // 		s.unusedsince = nanotime()
        // 	}
        // 	s.npreleased = 0
        //
        // 	// Coalesce with earlier, later spans.
        // 	p := (s.base() - h.arena_start) >> _PageShift
        // 	if p > 0 {
        // 		before := h.spans[p-1]
        // 		if before != nil && before.state == _MSpanFree {
        // 			// Now adjust s.
        // 			s.startAddr = before.startAddr
        // 			s.npages += before.npages
        // 			s.npreleased = before.npreleased // absorb released pages
        // 			s.needzero |= before.needzero
        // 			p -= before.npages
        // 			h.spans[p] = s
        // 			// The size is potentially changing so the treap needs to delete adjacent nodes and
        // 			// insert back as a combined node.
        // 			if h.isLargeSpan(before.npages) {
        // 				// We have a t, it is large so it has to be in the treap so we can remove it.
        // 				h.freelarge.removeSpan(before)
        // 			} else {
        // 				h.freeList(before.npages).remove(before)
        // 			}
        // 			before.state = _MSpanDead
        // 			h.spanalloc.free(unsafe.Pointer(before))
        // 		}
        // 	}
        //
        // 	// Now check to see if next (greater addresses) span is free and can be coalesced.
        // 	if (p + s.npages) < uintptr(len(h.spans)) {
        // 		after := h.spans[p+s.npages]
        // 		if after != nil && after.state == _MSpanFree {
        // 			s.npages += after.npages
        // 			s.npreleased += after.npreleased
        // 			s.needzero |= after.needzero
        // 			h.spans[p+s.npages-1] = s
        // 			if h.isLargeSpan(after.npages) {
        // 				h.freelarge.removeSpan(after)
        // 			} else {
        // 				h.freeList(after.npages).remove(after)
        // 			}
        // 			after.state = _MSpanDead
        // 			h.spanalloc.free(unsafe.Pointer(after))
        // 		}
        // 	}
        //
        // 	// Insert s into appropriate list or treap.
        // 	if h.isLargeSpan(s.npages) {
        // 		h.freelarge.insert(s)
        // 	} else {
        // 		h.freeList(s.npages).insert(s)
        // 	}
        // }
    }

    pub fn busy_list(&mut self, number_of_pages: usize) -> &mut MemorySpanList {
        if number_of_pages < self.busy.len() {
            return &mut self.busy[number_of_pages];
        }
        return &mut self.busy_large;
    }
}

// Main malloc heap.
// The heap itself is the "free[]" and "large" arrays,
// but all the other global data is here too.
//
pub struct MemoryHeap {
    pub protected: Mutex<ProtectedMemoryHeap>,
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
    // arena_start uintptr
    // arena_used  uintptr // Set with setArenaUsed.
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
    pub fn free_manual_span(&self, mut span: MemorySpan /*, stat *uint64 */) {
        unsafe { span.as_mut() }.need_zero = 1;

        let mut protected = self
            .protected
            .lock()
            .expect("Could not get memory heap lock");
        // *stat -= uint64(s.npages << _PageShift)
        // memstats.heap_sys += uint64(s.npages << _PageShift)
        protected.free_span(self, span, false, true, 0)
    }

    // Free the span back into the heap.
    pub fn free_span(&self, span: MemorySpan, account: i32) {
        let mut protected = self
            .protected
            .lock()
            .expect("Could not get memory heap lock");

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

        protected.free_span(self, span, true, true, 0)
    }
}

pub fn new_memory_heap() -> MemoryHeap {
    let memory_central: [CacheAligned<MemoryCentral>; NUM_SPAN_CLASSES.0 as usize] =
        array_init(|i| CacheAligned {
            0: MemoryCentral::new(),
        });
    let busy_span_lists: [MemorySpanList; MAX_MEMORY_HEAP_LIST] =
        array_init(|i| MemorySpanList::new());

    MemoryHeap {
        protected: Mutex::new(ProtectedMemoryHeap {
            pages_in_use: 0,
            busy: busy_span_lists,
            busy_large: MemorySpanList::new(),
        }),
        sweep_done: AtomicBool::new(false),
        sweepers: AtomicUsize::new(0),
        sweep_generation: AtomicUsize::new(0),
        sweep_buffers: [SweepBuffer::new(), SweepBuffer::new()],
        pages_swept: AtomicUsize::new(0),
        central: memory_central,
    }
}
