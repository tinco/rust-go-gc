use super::sweep_buffer::*;
use std::sync::atomic::{AtomicUsize, AtomicBool};


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
    pub sweep_generation:  AtomicUsize,       // sweep generation, see comment in mspan, is increased by 2 after every GC
    pub sweep_done: AtomicBool,              // all spans are swept
    pub sweepers: AtomicUsize,                 // number of active sweepone calls
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
    sweep_buffers: [SweepBuffer;2],

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
    pub fn get_pushable_sweep_buffer(&self, generation: usize) -> &PushableSweepBuffer {
        unsafe {
            std::mem::transmute::<
                &SweepBuffer,
                &PushableSweepBuffer>(&self.sweep_buffers[generation/2%2])
        }
    }

    pub fn get_popable_sweep_buffer(&self, generation: usize) -> &PopableSweepBuffer {
        unsafe {
            std::mem::transmute::<
                &SweepBuffer,
                &PopableSweepBuffer>(&self.sweep_buffers[1 - generation/2%2])
        }
    }
}

pub fn new_memory_heap() -> MemoryHeap {
    MemoryHeap {
        sweep_done: AtomicBool::new(false),
        sweepers: AtomicUsize::new(0),
        sweep_generation: AtomicUsize::new(0),
        sweep_buffers: [SweepBuffer::new(), SweepBuffer::new()],
    }
}
