use super::memory_allocator::*;
use super::size_classes::*;
use page_size;
use std::ptr::Unique;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use super::gc;

pub struct MemorySpanData {
    // next *mspan     // next span in list, or nil if none
    // prev *mspan     // previous span in list, or nil if none
    // list *mSpanList // For debugging. TODO: Remove.
    pub in_list: bool, // TODO replaces list ^
    //
    pub start_address: Unique<u8>, // address of first byte of span aka s.base()
    pub number_of_pages: usize,    // number of pages in span
    //
    // manualFreeList gclinkptr // list of free objects in _MSpanManual spans
    //
    // freeindex is the slot index between 0 and nelems at which to begin scanning
    // for the next free object in this span.
    // Each allocation scans allocBits starting at freeindex until it encounters a 0
    // indicating a free object. freeindex is then adjusted so that subsequent scans begin
    // just past the newly discovered free object.
    //
    // If freeindex == nelem, this span has no free objects.
    //
    // allocBits is a bitmap of objects in this span.
    // If n >= freeindex and allocBits[n/8] & (1<<(n%8)) is 0
    // then object n is free;
    // otherwise, object n is allocated. Bits starting at nelem are
    // undefined and should never be referenced.
    //
    // Object n starts at address n*elemsize + (start << pageShift).
    pub free_index: usize,
    // // TODO: Look up nelems from sizeclass and remove this field if it
    // // helps performance.
    pub number_of_elements: usize, // number of object in the span.
    //
    // Cache of the allocBits at freeindex. allocCache is shifted
    // such that the lowest bit corresponds to the bit freeindex.
    // allocCache holds the complement of allocBits, thus allowing
    // ctz (count trailing zero) to use it directly.
    // allocCache may contain bits beyond s.nelems; the caller must ignore
    // these.
    pub alloc_cache: u64,
    //
    // allocBits and gcmarkBits hold pointers to a span's mark and
    // allocation bits. The pointers are 8 byte aligned.
    // There are three arenas where this data is held.
    // free: Dirty arenas that are no longer accessed
    //       and can be reused.
    // next: Holds information to be used in the next GC cycle.
    // current: Information being used during this GC cycle.
    // previous: Information being used during the last GC cycle.
    // A new GC cycle starts with the call to finishsweep_m.
    // finishsweep_m moves the previous arena to the free arena,
    // the current arena to the previous arena, and
    // the next arena to the current arena.
    // The next arena is populated as the spans request
    // memory to hold gcmarkBits for the next GC cycle as well
    // as allocBits for newly allocated spans.

    // The pointer arithmetic is done "by hand" instead of using
    // arrays to avoid bounds checks along critical performance
    // paths. (TODO: NOT IN RUST: slice.get_unchecked())

    // The sweep will free the old allocBits and set allocBits to the
    // gcmarkBits. The gcmarkBits are replaced with a fresh zeroed
    // out memory.
    pub alloc_bits: Unique<u8>,
    pub gc_mark_bits: Unique<u8>,

    // // sweep generation:
    // // if sweepgen == h->sweepgen - 2, the span needs sweeping
    // // if sweepgen == h->sweepgen - 1, the span is currently being swept
    // // if sweepgen == h->sweepgen, the span is swept and ready to use
    // // h->sweepgen is incremented by 2 after every GC
    //
    pub sweep_generation: AtomicUsize, //TODO u32
    // divMul      uint16     // for divide by elemsize - divMagic.mul
    // baseMask    uint16     // if non-0, elemsize is a power of 2, & this will get object allocation base
    pub allocations_count: u16, // number of allocated objects
    pub span_class: SpanClass,  // size class and noscan (uint8)
    // incache     bool       // being used by an mcache
    pub state: State,  // mspaninuse etc
    pub need_zero: u8, // needs to be zeroed before allocation
    // divShift    uint8      // for divide by elemsize - divMagic.shift
    // divShift2   uint8      // for divide by elemsize - divMagic.shift2
    pub element_size: usize,   // computed from sizeclass or from npages
    pub unused_since: Instant, // first time spotted by gc in mspanfree state (note: on golang this is i64 instead of 128bit Instant)
    pub number_of_pages_released: usize, // number of pages released to the os
    pub scavenged: bool,       // whether this span has had its pages released to the OS

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
    InUse,  // allocated for garbage collected heap
    Manual, // allocated for manual management (e.g., stack allocator)
    Free,
}

// A spanClass represents the size class and noscan-ness of a span.
//
// Each size class has a noscan spanClass and a scan spanClass. The
// noscan spanClass contains only noscan objects, which do not contain
// pointers and thus do not need to be scanned by the garbage
// collector.
#[derive(Clone, Copy)]
pub struct SpanClass(pub u8);

impl SpanClass {
    pub fn size_class(self) -> i8 {
        (self.0 >> 1) as i8
    }

    pub fn no_scan(self) -> bool {
        (self.0 & 1) != 0
    }
}

pub const NUM_SPAN_CLASSES: SpanClass = SpanClass(NUM_SIZE_CLASSES << 1);
pub const TINY_SPAN_CLASS: SpanClass = SpanClass(TINY_SIZE_CLASS << 1 | 1);

// oneBitCount is indexed by byte and produces the
// number of 1 bits in that byte. For example 128 has 1 bit set
// and oneBitCount[128] will holds 1.
const ONE_BIT_COUNT: [u8; 256] = [
    0, 1, 1, 2, 1, 2, 2, 3, 1, 2, 2, 3, 2, 3, 3, 4, 1, 2, 2, 3, 2, 3, 3, 4, 2, 3, 3, 4, 3, 4, 4, 5,
    1, 2, 2, 3, 2, 3, 3, 4, 2, 3, 3, 4, 3, 4, 4, 5, 2, 3, 3, 4, 3, 4, 4, 5, 3, 4, 4, 5, 4, 5, 5, 6,
    1, 2, 2, 3, 2, 3, 3, 4, 2, 3, 3, 4, 3, 4, 4, 5, 2, 3, 3, 4, 3, 4, 4, 5, 3, 4, 4, 5, 4, 5, 5, 6,
    2, 3, 3, 4, 3, 4, 4, 5, 3, 4, 4, 5, 4, 5, 5, 6, 3, 4, 4, 5, 4, 5, 5, 6, 4, 5, 5, 6, 5, 6, 6, 7,
    1, 2, 2, 3, 2, 3, 3, 4, 2, 3, 3, 4, 3, 4, 4, 5, 2, 3, 3, 4, 3, 4, 4, 5, 3, 4, 4, 5, 4, 5, 5, 6,
    2, 3, 3, 4, 3, 4, 4, 5, 3, 4, 4, 5, 4, 5, 5, 6, 3, 4, 4, 5, 4, 5, 5, 6, 4, 5, 5, 6, 5, 6, 6, 7,
    2, 3, 3, 4, 3, 4, 4, 5, 3, 4, 4, 5, 4, 5, 5, 6, 3, 4, 4, 5, 4, 5, 5, 6, 4, 5, 5, 6, 5, 6, 6, 7,
    3, 4, 4, 5, 4, 5, 5, 6, 4, 5, 5, 6, 5, 6, 6, 7, 4, 5, 5, 6, 5, 6, 6, 7, 5, 6, 6, 7, 6, 7, 7, 8,
];

impl MemorySpanData {
    // Sweep frees or collects finalizers for blocks not marked in the mark phase.
    // It clears the mark bits in preparation for the next GC round.
    // Returns true if the span was returned to heap.
    // If preserve=true, don't return it to heap nor relink in MCentral lists;
    // caller takes care of it.
    //TODO go:nowritebarrier
    pub fn sweep(&mut self, gc: &mut gc::GC, preserve: bool) -> bool {
        // It's critical that we enter this function with preemption disabled,
        // GC must not start while we are in the middle of this function.
        // Since we're not preempting any Rust code, we don't have to worry about that.

        let sweep_generation = gc.memory_heap.sweep_generation.load(Ordering::Relaxed);
        let span_sweep_generation = self.sweep_generation.load(Ordering::Relaxed);

        if self.state != State::InUse || span_sweep_generation != sweep_generation - 1 {
            //print("MSpan_Sweep: state=", s.state, " sweepgen=", s.sweepgen, " mheap.sweepgen=", sweepgen, "\n")
            panic!("MemorySpan#sweep: bad span state");
        }

        // if trace.enabled {
        // 	traceGCSweepSpan(s.npages * _PageSize)
        // }

        gc.memory_heap
            .pages_swept
            .fetch_add(self.number_of_pages, Ordering::Relaxed);

        let span_class = self.span_class;
        let element_size = self.element_size;
        let mut result = false;

        // Unlink & free special records for any objects we're about to free.
        self.free_specials();
        self.maybe_trace_free();

        // The allocBits indicate which unmarked objects don't need to be
        // processed since they were free at the end of the last GC cycle
        // and were not allocated since then.
        // If the allocBits index is >= s.freeindex and the bit
        // is not marked then the object remains unallocated
        // since the last GC.
        // This situation is analogous to being on a freelist.

        // c := _g_.m.mcache
        let mut free_to_heap = false;

        // Count the number of free objects in this span.
        let number_of_allocations = self.count_allocations() as u16;
        if span_class.size_class() == 0 && number_of_allocations == 0 {
            self.need_zero = 1;
            free_to_heap = true;
        }

        let number_freed = self.allocations_count - number_of_allocations;
        if number_of_allocations > self.allocations_count {
            // print("runtime: nelems=", s.nelems, " nalloc=", nalloc, " previous allocCount=", s.allocCount, " nfreed=", nfreed, "\n")
            panic!("sweep increased allocation count")
        }

        self.allocations_count = number_of_allocations;

        let was_empty = self.next_free_index() == self.number_of_elements;

        // reset allocation index to start of span.
        self.free_index = 0;
        // if trace.enabled {
        // 	getg().m.p.ptr().traceReclaimed += uintptr(nfreed) * s.elemsize
        // }
        //
        // gcmarkBits becomes the allocBits.
        // get a fresh cleared gcmarkBits in preparation for next GC
        self.alloc_bits = self.gc_mark_bits;
        self.gc_mark_bits = gc.new_mark_bits(self.number_of_elements);

        // Initialize alloc bits cache.
        self.refill_allocations_cache(0);

        // We need to set s.sweepgen = h.sweepgen only when all blocks are swept,
        // because of the potential for a concurrent free/SetFinalizer.
        // But we need to set it before we make the span available for allocation
        // (return it to heap or mcentral), because allocation code assumes that a
        // span is already swept if available for allocation.
        if free_to_heap || number_freed == 0 {
            // The span must be in our exclusive ownership until we update sweepgen,
            // check for potential races.
            if self.state != State::InUse
                || self.sweep_generation.load(Ordering::Relaxed) != sweep_generation - 1
            {
                // print("MSpan_Sweep: state=", s.state, " sweepgen=", s.sweepgen, " mheap.sweepgen=", sweepgen, "\n")
                panic!("MSpan_Sweep: bad span state after sweep");
            }
            // Serialization point.
            // At this point the mark bits are cleared and allocation ready
            // to go so release the span.
            self.sweep_generation
                .store(sweep_generation, Ordering::Relaxed);
        }

        if number_freed > 0 && span_class.size_class() != 0 {
            // TODO accounting: c.local_nsmallfree[spc.sizeclass()] += uintptr(nfreed)
            // TODO that we create a pointer with unchecked ownership of self in all of the below
            // branches, probably means this whole function should take ownership.
            // Maybe change the signature of this function to convey the transfer of ownership?
            let unique_self = unsafe { Unique::new_unchecked(self) };
            result = gc.memory_heap.central[span_class.0 as usize].0.free_span(
                unique_self,
                preserve,
                was_empty,
            )
        // MCentral_FreeSpan updates sweepgen
        } else if free_to_heap {
            // Free large span to heap

            // NOTE(rsc,dvyukov): The original implementation of efence
            // in CL 22060046 used SysFree instead of SysFault, so that
            // the operating system would eventually give the memory
            // back to us again, so that an efence program could run
            // longer without running out of memory. Unfortunately,
            // calling SysFree here without any kind of adjustment of the
            // heap data structures means that when the memory does
            // come back to us, we have the wrong metadata for it, either in
            // the MSpan structures or in the garbage collection bitmap.
            // Using SysFault here means that the program will run out of
            // memory fairly quickly in efence mode, but at least it won't
            // have mysterious crashes due to confused memory reuse.
            // It should be possible to switch back to SysFree if we also
            // implement and then call some kind of MHeap_DeleteSpan.
            // TODO: maybe implement efence (https://linux.die.net/man/3/efence) as well?
            // if debug.efence > 0 {
            // 	s.limit = 0 // prevent mlookup from finding this span
            // 	sysFault(unsafe.Pointer(s.base()), size)
            // } else {
            let unique_self = unsafe { Unique::new_unchecked(self) };
            gc.memory_heap.free_span(unique_self, 1);
            // }
            // TODO implement accounting
            // c.local_nlargefree++
            // c.local_largefree += size
            result = true;
        }
        if !result {
            // The span has been swept and is still in-use, so put
            // it on the swept in-use list.
            let unique_self = unsafe { Unique::new_unchecked(self) };
            gc.memory_heap
                .get_pushable_sweep_buffer(sweep_generation)
                .push(unique_self);
        }
        result
    }

    pub fn is_in_list(&self) -> bool {
        self.in_list
    }

    pub fn base(&self) -> Unique<u8> {
        return self.start_address;
    }

    // Unlink & free special records for any objects we're about to free.
    // Two complications here:
    // 1. An object can have both finalizer and profile special records.
    //    In such case we need to queue finalizer for execution,
    //    mark the object as live and preserve the profile special.
    // 2. A tiny object can have several finalizers setup for different offsets.
    //    If such object is not marked, we need to queue all finalizers at once.
    // Both 1 and 2 are possible at the same time.
    fn free_specials(&mut self) {
        unimplemented!();
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
    }

    // TODO this one is not functionally necessary I think
    fn maybe_trace_free(&mut self) {
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
    }

    // countAlloc returns the number of objects allocated in span s by
    // scanning the allocation bitmap.
    // TODO:(rlh) Use popcount intrinsic.
    fn count_allocations(&mut self) -> usize {
        let mut count: usize = 0;
        let max_index = (self.number_of_elements / 8) as isize;
        for i in 0..max_index {
            let mark_bits = unsafe { *self.gc_mark_bits.as_ptr().offset(i) } as usize;
            count += ONE_BIT_COUNT[mark_bits] as usize;
        }
        let bits_in_last_byte = self.number_of_elements % 8;
        if bits_in_last_byte != 0 {
            let mark_bits = unsafe { *self.gc_mark_bits.as_ptr().offset(max_index) };
            let mask = (1 << bits_in_last_byte) - 1;
            let bits = mark_bits & mask;
            count += ONE_BIT_COUNT[bits as usize] as usize;
        }
        count
    }

    // nextFreeIndex returns the index of the next free object in s at
    // or after s.freeindex.
    // There are hardware instructions that can be used to make this
    // faster if profiling warrants it.
    fn next_free_index(&mut self) -> usize {
        let mut span_free_index = self.free_index;
        let span_number_of_elements = self.number_of_elements;

        if span_free_index == span_number_of_elements {
            return span_free_index;
        }

        if span_free_index > span_number_of_elements {
            panic!("span.free_index > span.number_of_elements");
        }

        let alloc_cache = self.alloc_cache;
        let mut bit_index = alloc_cache.trailing_zeros();
        while bit_index == 64 {
            // Move index to start of next cached bits.
            span_free_index = (span_free_index + 64) & !(64 - 1); //TODO what is !(64-1)?
            if span_free_index >= span_number_of_elements {
                self.free_index = span_number_of_elements;
                return span_number_of_elements;
            }

            let which_byte = span_free_index / 8;
            // Refill s.allocCache with the next 64 alloc bits.
            self.refill_allocations_cache(which_byte);

            let alloc_cache = self.alloc_cache;
            bit_index = alloc_cache.trailing_zeros();
            // nothing available in cached bits
            // grab the next 8 bytes and try again.
        }
        let result = span_free_index + (bit_index as usize);
        if result >= span_number_of_elements {
            self.free_index = span_number_of_elements;
            return span_number_of_elements;
        }

        self.alloc_cache >>= (bit_index + 1) as usize;
        span_free_index = result + 1;

        if span_free_index % 64 == 0 && span_free_index != span_number_of_elements {
            // We just incremented self.free_index so it isn't 0.
            // As each 1 in self.alloc_cache was encountered and used for allocation
            // it was shifted away. At this point self.alloc_cache contains all 0s.
            // Refill self.alloc_cache so that it corresponds
            // to the bits at s.allocBits starting at s.freeindex.
            let which_byte = span_free_index / 8;
            self.refill_allocations_cache(which_byte);
        }
        self.free_index = span_free_index;
        result
    }

    // refill_allocations_cache takes 8 bytes s.allocBits starting at which_byte
    // and negates them so that ctz (count trailing zeros) instructions
    // can be used. It then places these 8 bytes into the cached 64 bit
    // span.alloc_cache.
    fn refill_allocations_cache(&mut self, which_byte: usize) {
        let bytes =
            unsafe { *(self.alloc_bits.as_ptr().offset(which_byte as isize) as *const [u8; 8]) };
        let alloc_cache = u64::from_le_bytes(bytes);
        self.alloc_cache = !alloc_cache;
    }

    // released returns the number of bytes in this span
    // which were returned back to the OS.
    pub fn released(&mut self) -> usize {
        if !self.scavenged {
            return 0;
        }
        let (start, end) = self.physical_page_bounds();
        return end - start;
    }

    // physPageBounds returns the start and end of the span
    // rounded in to the physical page size.
    fn physical_page_bounds(&mut self) -> (usize, usize) {
        let mut start = self.base().as_ptr() as usize;
        let mut end = start + self.number_of_pages << PAGE_SHIFT;
        let physical_page_size = page_size::get();
        if physical_page_size > PAGE_SIZE {
            // Round start and end in.
            start = (start + physical_page_size - 1) & !(physical_page_size - 1);
            end &= !(physical_page_size - 1);
        }
        return (start, end);
    }
}
