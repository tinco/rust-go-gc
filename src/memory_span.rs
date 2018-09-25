use std::ptr::Unique;
use std::sync::atomic::{AtomicUsize};

pub struct MemorySpanData {
    // next *mspan     // next span in list, or nil if none
    // prev *mspan     // previous span in list, or nil if none
    // list *mSpanList // For debugging. TODO: Remove.
    //
    // startAddr uintptr // address of first byte of span aka s.base()
    pub number_of_pages:    usize, // number of pages in span
    //
    // manualFreeList gclinkptr // list of free objects in _MSpanManual spans
    //
    // // freeindex is the slot index between 0 and nelems at which to begin scanning
    // // for the next free object in this span.
    // // Each allocation scans allocBits starting at freeindex until it encounters a 0
    // // indicating a free object. freeindex is then adjusted so that subsequent scans begin
    // // just past the newly discovered free object.
    // //
    // // If freeindex == nelem, this span has no free objects.
    // //
    // // allocBits is a bitmap of objects in this span.
    // // If n >= freeindex and allocBits[n/8] & (1<<(n%8)) is 0
    // // then object n is free;
    // // otherwise, object n is allocated. Bits starting at nelem are
    // // undefined and should never be referenced.
    // //
    // // Object n starts at address n*elemsize + (start << pageShift).
    // freeindex uintptr
    // // TODO: Look up nelems from sizeclass and remove this field if it
    // // helps performance.
    // nelems uintptr // number of object in the span.
    //
    // // Cache of the allocBits at freeindex. allocCache is shifted
    // // such that the lowest bit corresponds to the bit freeindex.
    // // allocCache holds the complement of allocBits, thus allowing
    // // ctz (count trailing zero) to use it directly.
    // // allocCache may contain bits beyond s.nelems; the caller must ignore
    // // these.
    // allocCache uint64
    //
    // // allocBits and gcmarkBits hold pointers to a span's mark and
    // // allocation bits. The pointers are 8 byte aligned.
    // // There are three arenas where this data is held.
    // // free: Dirty arenas that are no longer accessed
    // //       and can be reused.
    // // next: Holds information to be used in the next GC cycle.
    // // current: Information being used during this GC cycle.
    // // previous: Information being used during the last GC cycle.
    // // A new GC cycle starts with the call to finishsweep_m.
    // // finishsweep_m moves the previous arena to the free arena,
    // // the current arena to the previous arena, and
    // // the next arena to the current arena.
    // // The next arena is populated as the spans request
    // // memory to hold gcmarkBits for the next GC cycle as well
    // // as allocBits for newly allocated spans.
    // //
    // The pointer arithmetic is done "by hand" instead of using
    // arrays to avoid bounds checks along critical performance
    // paths. (TODO: NOT IN RUST: slice.get_unchecked())
    // // The sweep will free the old allocBits and set allocBits to the
    // // gcmarkBits. The gcmarkBits are replaced with a fresh zeroed
    // // out memory.
    // allocBits  *gcBits
    // gcmarkBits *gcBits
    //
    // // sweep generation:
    // // if sweepgen == h->sweepgen - 2, the span needs sweeping
    // // if sweepgen == h->sweepgen - 1, the span is currently being swept
    // // if sweepgen == h->sweepgen, the span is swept and ready to use
    // // h->sweepgen is incremented by 2 after every GC
    //
    pub sweep_generation:  AtomicUsize, //TODO u32
    // divMul      uint16     // for divide by elemsize - divMagic.mul
    // baseMask    uint16     // if non-0, elemsize is a power of 2, & this will get object allocation base
    // allocCount  uint16     // number of allocated objects
    // spanclass   spanClass  // size class and noscan (uint8)
    // incache     bool       // being used by an mcache
    pub state:       State, // mspaninuse etc
    // needzero    uint8      // needs to be zeroed before allocation
    // divShift    uint8      // for divide by elemsize - divMagic.shift
    // divShift2   uint8      // for divide by elemsize - divMagic.shift2
    // elemsize    uintptr    // computed from sizeclass or from npages
    // unusedsince int64      // first time spotted by gc in mspanfree state
    // npreleased  uintptr    // number of pages released to the os
    // limit       uintptr    // end of data in span
    // speciallock mutex      // guards specials list
    // specials    *special   // linked list of special records sorted by offset.
}

pub type MemorySpan = Unique<MemorySpanData>;

// An MSpan representing actual memory has state _MSpanInUse,
// _MSpanManual, or _MSpanFree. Transitions between these states are
// constrained as follows:
//
// * A span may transition from free to in-use or manual during any GC
//   phase.
//
// * During sweeping (gcphase == _GCoff), a span may transition from
//   in-use to free (as a result of sweeping) or manual to free (as a
//   result of stacks being freed).
//
// * During GC (gcphase != _GCoff), a span *must not* transition from
//   manual or in-use to free. Because concurrent GC may read a pointer
//   and then look up its span, the span state must be monotonic.
#[derive(Debug, PartialEq, Eq)]
pub enum State {
    Dead,
    InUse,   // allocated for garbage collected heap
    Manual,  // allocated for manual management (e.g., stack allocator)
    Free
}

impl MemorySpanData {
    // Sweep frees or collects finalizers for blocks not marked in the mark phase.
    // It clears the mark bits in preparation for the next GC round.
    // Returns true if the span was returned to heap.
    // If preserve=true, don't return it to heap nor relink in MCentral lists;
    // caller takes care of it.
    //TODO go:nowritebarrier
    pub fn sweep(&mut self, preserve: bool) -> bool {
        // It's critical that we enter this function with preemption disabled,
        // GC must not start while we are in the middle of this function.
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
        false
    }

}
