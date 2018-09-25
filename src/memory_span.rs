//go:notinheap
pub struct MemorySpanData {
	// next *mspan     // next span in list, or nil if none
	// prev *mspan     // previous span in list, or nil if none
	// list *mSpanList // For debugging. TODO: Remove.
    //
	// startAddr uintptr // address of first byte of span aka s.base()
	// npages    uintptr // number of pages in span
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
	// sweepgen    uint32
	// divMul      uint16     // for divide by elemsize - divMagic.mul
	// baseMask    uint16     // if non-0, elemsize is a power of 2, & this will get object allocation base
	// allocCount  uint16     // number of allocated objects
	// spanclass   spanClass  // size class and noscan (uint8)
	// incache     bool       // being used by an mcache
	// state       mSpanState // mspaninuse etc
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

pub type MemorySpan = &'static MemorySpanData;
