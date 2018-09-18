// Copyright 2009 The Go Authors. All rights reserved.
// Use of this source code is governed by a BSD-style
// license that can be found in the LICENSE file.
// Copyright 2018 Tinco Andringa.

#![feature(async_await)]
#![feature(futures_api)]
#![feature(await_macro)]
#![feature(pin)]
#![feature(arbitrary_self_types)]
#![feature(generators)]

mod sweep_buffer;

use futures::sync::{oneshot, mpsc};
use futures::stream::Stream;
use futures_future::futures_future;
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicUsize, AtomicBool, Ordering};


// Main malloc heap.
// The heap itself is the "free[]" and "large" arrays,
// but all the other global data is here too.
//
// mheap must not be heap-allocated because it contains mSpanLists,
// which must not be heap-allocated.
//
//go:notinheap
pub struct MemoryHeap {
    // lock      mutex
    // free      [_MaxMHeapList]mSpanList // free lists of given length up to _MaxMHeapList
    // freelarge mTreap                   // free treap of length >= _MaxMHeapList
    // busy      [_MaxMHeapList]mSpanList // busy lists of large spans of given length
    // busylarge mSpanList                // busy lists of large spans length >= _MaxMHeapList
    // TODO these should all be u32 for locality optimization?
    sweep_generation:  AtomicUsize,       // sweep generation, see comment in mspan, is increased by 2 after every GC
    sweep_done: AtomicBool,              // all spans are swept
    sweepers: AtomicUsize,                 // number of active sweepone calls
    //
    // // allspans is a slice of all mspans ever created. Each mspan
    // // appears exactly once.
    // //
    // // The memory for allspans is manually managed and can be
    // // reallocated and move as the heap grows.
    // //
    // // In general, allspans is protected by mheap_.lock, which
    // // prevents concurrent access as well as freeing the backing
    // // store. Accesses during STW might not hold the lock, but
    // // must ensure that allocation cannot happen around the
    // // access (since that may free the backing store).
    // allspans []*mspan // all spans out there
    //
    // // spans is a lookup table to map virtual address page IDs to *mspan.
    // // For allocated spans, their pages map to the span itself.
    // // For free spans, only the lowest and highest pages map to the span itself.
    // // Internal pages map to an arbitrary span.
    // // For pages that have never been allocated, spans entries are nil.
    // //
    // // This is backed by a reserved region of the address space so
    // // it can grow without moving. The memory up to len(spans) is
    // // mapped. cap(spans) indicates the total reserved memory.
    // spans []*mspan
    //

    // sweep_buffers contains two mspan stacks: one of swept in-use
    // spans, and one of unswept in-use spans. These two trade
    // roles on each GC cycle. Since the sweepgen increases by 2
    // on each cycle, this means the swept spans are in
    // sweep_buffers[sweepgen/2%2] and the unswept spans are in
    // sweep_buffers[1-sweepgen/2%2]. Sweeping pops spans from the
    // unswept stack and pushes spans that are still in-use on the
    // swept stack. Likewise, allocating an in-use span pushes it
    // on the swept stack.
    sweep_buffers: [sweep_buffer::SweepBuffer;2],

    //
    // _ uint32 // align uint64 fields on 32-bit for atomics
    //
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
    // pagesInUse         uint64  // pages of spans in stats _MSpanInUse; R/W with mheap.lock
    // pagesSwept         uint64  // pages swept this cycle; updated atomically
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
    // // central free lists for small size classes.
    // // the padding makes sure that the MCentrals are
    // // spaced CacheLineSize bytes apart, so that each MCentral.lock
    // // gets its own cache line.
    // // central is indexed by spanClass.
    // central [numSpanClasses]struct {
    // 	mcentral mcentral
    // 	pad      [sys.CacheLineSize - unsafe.Sizeof(mcentral{})%sys.CacheLineSize]byte
    // }
    //
    // spanalloc             fixalloc // allocator for span*
    // cachealloc            fixalloc // allocator for mcache*
    // treapalloc            fixalloc // allocator for treapNodes* used by large objects
    // specialfinalizeralloc fixalloc // allocator for specialfinalizer*
    // specialprofilealloc   fixalloc // allocator for specialprofile*
    // speciallock           mutex    // lock for special record allocators.
}

impl MemoryHeap {
    pub fn get_pushable_sweep_buffer(&self, generation: usize) -> &sweep_buffer::PushableSweepBuffer {
        unsafe {
            std::mem::transmute::<
                &sweep_buffer::SweepBuffer,
                &sweep_buffer::PushableSweepBuffer>(&self.sweep_buffers[generation/2%2])
        }
    }

    pub fn get_popable_sweep_buffer(&self, generation: usize) -> &sweep_buffer::PopableSweepBuffer {
        unsafe {
            std::mem::transmute::<
                &sweep_buffer::SweepBuffer,
                &sweep_buffer::PopableSweepBuffer>(&self.sweep_buffers[1 - generation/2%2])
        }
    }
}

pub fn new_memory_heap() -> MemoryHeap {
    MemoryHeap {
        sweep_done: AtomicBool::new(false),
        sweepers: AtomicUsize::new(0),
        sweep_generation: AtomicUsize::new(0),
        sweep_buffers: [sweep_buffer::SweepBuffer{}, sweep_buffer::SweepBuffer{}],
    }
}

// State of background sweep.
pub struct BackgroundSweep {
    parked: Mutex<bool>,
    // started: bool,

    // We can't background sweep until the gc is ready
    unpark_sender: Mutex<Option<mpsc::Sender<bool>>>,

    // bookkeeping numbers
    //TODO in Go these are regular u32's, but they're being modified without
    // a lock. I think this is safe because they're only for bookkeeping, but
    // I do think it means there's a potential race condition there in Go..
    number: AtomicUsize,
    sweep_pauses: AtomicUsize,
}

impl BackgroundSweep {
    fn wait_for_unpark(&self) -> mpsc::Receiver<bool> {
        let (unpark_sender, unpark_receiver) = mpsc::channel::<bool>(0);
        let mut sender_field = self.unpark_sender.lock()
            .expect("Could not get ready sender lock");
        *sender_field = Some(unpark_sender);
        unpark_receiver
    }
}

pub fn new_sweep() -> BackgroundSweep {
    BackgroundSweep {
        parked: Mutex::new(false),
        // started: false,
        unpark_sender: Mutex::new(None),
        number: AtomicUsize::new(0),
        sweep_pauses: AtomicUsize::new(0),
    }
}

pub struct GC {
    background_sweep: BackgroundSweep,
    memory_heap: MemoryHeap,
}

pub fn new_gc() -> GC {
    GC {
        background_sweep: new_sweep(),
        memory_heap: new_memory_heap(),
    }
}

impl GC {
    // gcenable is called after the bulk of the runtime initialization,
    // just before we're about to start letting user code run.
    // It kicks off the background sweeper routine and enables GC.
    pub async fn enable(&mut self) {
        let (signal_setup_done, mut setup_done) = oneshot::channel::<bool>();

        self.background_sweep(signal_setup_done);

        let f = futures_future(&mut setup_done);

        await!(f);
    }

    async fn background_sweep(&mut self, signal_setup_done: oneshot::Sender<bool>) {
        // This assignment is to a block, so that the lock is released at the end of it and
        // we can safely wait for something to signal us that the gc is ready.
        let unpark = {
            let mut sweep_parked = self.background_sweep.parked.lock().expect("Could not get lock on SweepData");
            *sweep_parked = true;

            let _ = signal_setup_done.send(true);

            self.background_sweep.wait_for_unpark()
        };

        let mut unpark_future = unpark.into_future();
        await!(futures_future(&mut unpark_future));

        loop {
            // we sweep one and then yield to other goroutines until sweepone returns FFFFFFF
            while self.sweep_one() != !0 {
                self.background_sweep.number.fetch_add(1, Ordering::Relaxed);
                yield
            }

            while self.free_some_work_buffers(true) { yield }

            let mut sweep_parked = self.background_sweep.parked.lock()
                .expect("Could not get background sweeper parked lock");

        	if !self.memory_heap.sweep_done.load(Ordering::Relaxed) {
        		// This can happen if a GC runs between
        		// sweep_one returning !0 above
        		// and the lock being acquired.
        		continue
        	}

            *sweep_parked = true;
            await!(futures_future(&mut unpark_future));
        }
    }

    // freeSomeWbufs frees some workbufs back to the heap and returns
    // true if it should be called again to free more.
    fn free_some_work_buffers(&self, preemptible: bool) -> bool {
    	let batch_size = 64; // ~1–2 µs per span.
    	// lock(&work.wbufSpans.lock)
    	// if gcphase != _GCoff || work.wbufSpans.free.isEmpty() {
    	// 	unlock(&work.wbufSpans.lock)
    	// 	return false
    	// }
    	// systemstack(func() {
    	// 	gp := getg().m.curg
    	// 	for i := 0; i < batchSize && !(preemptible && gp.preempt); i++ {
    	// 		span := work.wbufSpans.free.first
    	// 		if span == nil {
    	// 			break
    	// 		}
    	// 		work.wbufSpans.free.remove(span)
    	// 		mheap_.freeManual(span, &memstats.gc_sys)
    	// 	}
    	// })
    	// more := !work.wbufSpans.free.isEmpty()
    	// unlock(&work.wbufSpans.lock)
    	// return more
        false
    }


    // sweeps one span
    // returns number of pages returned to heap, or !0 if there is nothing to sweep
    //go:nowritebarrier
    pub fn sweep_one(&mut self) -> usize {
        // sweepRatio := mheap_.sweepPagesPerByte // For debugging

        // Go has preemption at every function invocation, so even during garbage collection
        // other goroutines might be preempt the current thread. To prevent that Go increases
        // the locks field on the native thread. Since in Rust we can't be preempted inside a
        // thread we don't have to worry about that.

        // TODO: I think go:nowritebarrier means the ordering here can be Relaxed, is that correct?
        return if self.memory_heap.sweep_done.load(Ordering::Relaxed) {
            !0
        } else {
            self.memory_heap.sweepers.fetch_add(1, Ordering::Relaxed);
            let number_of_pages : usize = !0;
            let sweep_generation = self.memory_heap.sweep_generation.load(Ordering::Relaxed);

            loop {
                // let sweep = self.memory_heap.sweep_spans.get(1 - sweep_generation / 2 % 2).pop();
                // 	if s == nil {
                // 		atomic.Store(&mheap_.sweepdone, 1)
                // 		break
                // 	}
                // 	if s.state != mSpanInUse {
                // 		// This can happen if direct sweeping already
                // 		// swept this span, but in that case the sweep
                // 		// generation should always be up-to-date.
                // 		if s.sweepgen != sg {
                // 			print("runtime: bad span s.state=", s.state, " s.sweepgen=", s.sweepgen, " sweepgen=", sg, "\n")
                // 			throw("non in-use span in unswept list")
                // 		}
                // 		continue
                // 	}
                // 	if s.sweepgen != sg-2 || !atomic.Cas(&s.sweepgen, sg-2, sg-1) {
                // 		continue
                // 	}
                // 	npages = s.npages
                // 	if !s.sweep(false) {
                // 		// Span is still in-use, so this returned no
                // 		// pages to the heap and the span needs to
                // 		// move to the swept in-use list.
                // 		npages = 0
                // 	}
                // 	break
            }
            //
            // // Decrement the number of active sweepers and if this is the
            // // last one print trace information.
            // if atomic.Xadd(&mheap_.sweepers, -1) == 0 && atomic.Load(&mheap_.sweepdone) != 0 {
            // 	if debug.gcpacertrace > 0 {
            // 		print("pacer: sweep done at heap size ", memstats.heap_live>>20, "MB; allocated ", (memstats.heap_live-mheap_.sweepHeapLiveBasis)>>20, "MB during sweep; swept ", mheap_.pagesSwept, " pages at ", sweepRatio, " pages/byte\n")
            // 	}
            // }
            // _g_.m.locks--
            // return npages

            number_of_pages
        }

    }
}

// Sweep frees or collects finalizers for blocks not marked in the mark phase.
// It clears the mark bits in preparation for the next GC round.
// Returns true if the span was returned to heap.
// If preserve=true, don't return it to heap nor relink in MCentral lists;
// caller takes care of it.
//TODO go:nowritebarrier
pub fn sweep(/* span this */ _preserve: bool) -> bool {
    false
    // // It's critical that we enter this function with preemption disabled,
    // // GC must not start while we are in the middle of this function.
    // _g_ := getg()
    // if _g_.m.locks == 0 && _g_.m.mallocing == 0 && _g_ != _g_.m.g0 {
    // 	throw("MSpan_Sweep: m is not locked")
    // }
    // sweepgen := mheap_.sweepgen
    // if s.state != mSpanInUse || s.sweepgen != sweepgen-1 {
    // 	print("MSpan_Sweep: state=", s.state, " sweepgen=", s.sweepgen, " mheap.sweepgen=", sweepgen, "\n")
    // 	throw("MSpan_Sweep: bad span state")
    // }
    //
    // if trace.enabled {
    // 	traceGCSweepSpan(s.npages * _PageSize)
    // }
    //
    // atomic.Xadd64(&mheap_.pagesSwept, int64(s.npages))
    //
    // spc := s.spanclass
    // size := s.elemsize
    // res := false
    //
    // c := _g_.m.mcache
    // freeToHeap := false
    //
    // // The allocBits indicate which unmarked objects don't need to be
    // // processed since they were free at the end of the last GC cycle
    // // and were not allocated since then.
    // // If the allocBits index is >= s.freeindex and the bit
    // // is not marked then the object remains unallocated
    // // since the last GC.
    // // This situation is analogous to being on a freelist.
    //
    // // Unlink & free special records for any objects we're about to free.
    // // Two complications here:
    // // 1. An object can have both finalizer and profile special records.
    // //    In such case we need to queue finalizer for execution,
    // //    mark the object as live and preserve the profile special.
    // // 2. A tiny object can have several finalizers setup for different offsets.
    // //    If such object is not marked, we need to queue all finalizers at once.
    // // Both 1 and 2 are possible at the same time.
    // specialp := &s.specials
    // special := *specialp
    // for special != nil {
    // 	// A finalizer can be set for an inner byte of an object, find object beginning.
    // 	objIndex := uintptr(special.offset) / size
    // 	p := s.base() + objIndex*size
    // 	mbits := s.markBitsForIndex(objIndex)
    // 	if !mbits.isMarked() {
    // 		// This object is not marked and has at least one special record.
    // 		// Pass 1: see if it has at least one finalizer.
    // 		hasFin := false
    // 		endOffset := p - s.base() + size
    // 		for tmp := special; tmp != nil && uintptr(tmp.offset) < endOffset; tmp = tmp.next {
    // 			if tmp.kind == _KindSpecialFinalizer {
    // 				// Stop freeing of object if it has a finalizer.
    // 				mbits.setMarkedNonAtomic()
    // 				hasFin = true
    // 				break
    // 			}
    // 		}
    // 		// Pass 2: queue all finalizers _or_ handle profile record.
    // 		for special != nil && uintptr(special.offset) < endOffset {
    // 			// Find the exact byte for which the special was setup
    // 			// (as opposed to object beginning).
    // 			p := s.base() + uintptr(special.offset)
    // 			if special.kind == _KindSpecialFinalizer || !hasFin {
    // 				// Splice out special record.
    // 				y := special
    // 				special = special.next
    // 				*specialp = special
    // 				freespecial(y, unsafe.Pointer(p), size)
    // 			} else {
    // 				// This is profile record, but the object has finalizers (so kept alive).
    // 				// Keep special record.
    // 				specialp = &special.next
    // 				special = *specialp
    // 			}
    // 		}
    // 	} else {
    // 		// object is still live: keep special record
    // 		specialp = &special.next
    // 		special = *specialp
    // 	}
    // }
    //
    // if debug.allocfreetrace != 0 || raceenabled || msanenabled {
    // 	// Find all newly freed objects. This doesn't have to
    // 	// efficient; allocfreetrace has massive overhead.
    // 	mbits := s.markBitsForBase()
    // 	abits := s.allocBitsForIndex(0)
    // 	for i := uintptr(0); i < s.nelems; i++ {
    // 		if !mbits.isMarked() && (abits.index < s.freeindex || abits.isMarked()) {
    // 			x := s.base() + i*s.elemsize
    // 			if debug.allocfreetrace != 0 {
    // 				tracefree(unsafe.Pointer(x), size)
    // 			}
    // 			if raceenabled {
    // 				racefree(unsafe.Pointer(x), size)
    // 			}
    // 			if msanenabled {
    // 				msanfree(unsafe.Pointer(x), size)
    // 			}
    // 		}
    // 		mbits.advance()
    // 		abits.advance()
    // 	}
    // }
    //
    // // Count the number of free objects in this span.
    // nalloc := uint16(s.countAlloc())
    // if spc.sizeclass() == 0 && nalloc == 0 {
    // 	s.needzero = 1
    // 	freeToHeap = true
    // }
    // nfreed := s.allocCount - nalloc
    // if nalloc > s.allocCount {
    // 	print("runtime: nelems=", s.nelems, " nalloc=", nalloc, " previous allocCount=", s.allocCount, " nfreed=", nfreed, "\n")
    // 	throw("sweep increased allocation count")
    // }
    //
    // s.allocCount = nalloc
    // wasempty := s.nextFreeIndex() == s.nelems
    // s.freeindex = 0 // reset allocation index to start of span.
    // if trace.enabled {
    // 	getg().m.p.ptr().traceReclaimed += uintptr(nfreed) * s.elemsize
    // }
    //
    // // gcmarkBits becomes the allocBits.
    // // get a fresh cleared gcmarkBits in preparation for next GC
    // s.allocBits = s.gcmarkBits
    // s.gcmarkBits = newMarkBits(s.nelems)
    //
    // // Initialize alloc bits cache.
    // s.refillAllocCache(0)
    //
    // // We need to set s.sweepgen = h.sweepgen only when all blocks are swept,
    // // because of the potential for a concurrent free/SetFinalizer.
    // // But we need to set it before we make the span available for allocation
    // // (return it to heap or mcentral), because allocation code assumes that a
    // // span is already swept if available for allocation.
    // if freeToHeap || nfreed == 0 {
    // 	// The span must be in our exclusive ownership until we update sweepgen,
    // 	// check for potential races.
    // 	if s.state != mSpanInUse || s.sweepgen != sweepgen-1 {
    // 		print("MSpan_Sweep: state=", s.state, " sweepgen=", s.sweepgen, " mheap.sweepgen=", sweepgen, "\n")
    // 		throw("MSpan_Sweep: bad span state after sweep")
    // 	}
    // 	// Serialization point.
    // 	// At this point the mark bits are cleared and allocation ready
    // 	// to go so release the span.
    // 	atomic.Store(&s.sweepgen, sweepgen)
    // }
    //
    // if nfreed > 0 && spc.sizeclass() != 0 {
    // 	c.local_nsmallfree[spc.sizeclass()] += uintptr(nfreed)
    // 	res = mheap_.central[spc].mcentral.freeSpan(s, preserve, wasempty)
    // 	// MCentral_FreeSpan updates sweepgen
    // } else if freeToHeap {
    // 	// Free large span to heap
    //
    // 	// NOTE(rsc,dvyukov): The original implementation of efence
    // 	// in CL 22060046 used SysFree instead of SysFault, so that
    // 	// the operating system would eventually give the memory
    // 	// back to us again, so that an efence program could run
    // 	// longer without running out of memory. Unfortunately,
    // 	// calling SysFree here without any kind of adjustment of the
    // 	// heap data structures means that when the memory does
    // 	// come back to us, we have the wrong metadata for it, either in
    // 	// the MSpan structures or in the garbage collection bitmap.
    // 	// Using SysFault here means that the program will run out of
    // 	// memory fairly quickly in efence mode, but at least it won't
    // 	// have mysterious crashes due to confused memory reuse.
    // 	// It should be possible to switch back to SysFree if we also
    // 	// implement and then call some kind of MHeap_DeleteSpan.
    // 	if debug.efence > 0 {
    // 		s.limit = 0 // prevent mlookup from finding this span
    // 		sysFault(unsafe.Pointer(s.base()), size)
    // 	} else {
    // 		mheap_.freeSpan(s, 1)
    // 	}
    // 	c.local_nlargefree++
    // 	c.local_largefree += size
    // 	res = true
    // }
    // if !res {
    // 	// The span has been swept and is still in-use, so put
    // 	// it on the swept in-use list.
    // 	mheap_.sweepSpans[sweepgen/2%2].push(s)
    // }
    // return res
}


#[test]
fn it_works() {
}
