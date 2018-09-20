
use super::memory_span::*;
use std::sync::atomic::{AtomicUsize, AtomicPtr, Ordering};
use std::sync::{Mutex};
use std::alloc::*;
use std::mem;

const GC_SWEEP_BLOCK_ENTRIES: usize = 512; // 4KB on 64-bit
const GC_SWEEP_BUF_INIT_SPINE_CAP: usize = 256; // Enough for 1GB heap on 64-bit

pub struct SweepBuffer {
    // A SweepBuffer is a two-level data structure consisting of a
	// growable spine that points to fixed-sized blocks. The spine
	// can be accessed without locks, but adding a block or
	// growing it requires taking the spine lock.
	//
	// Because each MemorySpan covers at least 8K of heap and takes at
	// most 8 bytes in the SweepBuffer, the growth of the spine is
	// quite limited.
	//
	// The spine and all blocks are allocated off-heap, which
	// allows this to be used in the memory manager and avoids the
	// need for write barriers on all of these. We never release
	// this memory because there could be concurrent lock-free
	// access and we're likely to reuse it anyway. (In principle,
	// we could do this during STW.)

	// spineLock mutex
	// spine     unsafe.Pointer // *[N]*gcSweepBlock, accessed atomically
    //
	// // index is the first unused slot in the logical concatenation
	// // of all blocks. It is accessed atomically.
    pub index: AtomicUsize,
    pub spine_length: AtomicUsize, // Spine array length, accessed atomically
    pub spine_cap: Mutex<usize>, // Spine array cap, accessed under lock
    pub spine: AtomicPtr<SweepBlock>, // TODO write persistentalloc for possibly more performance
}

pub struct SweepBlockData {
	spans: [MemorySpan; GC_SWEEP_BLOCK_ENTRIES]
}

pub type SweepBlock = &'static mut SweepBlockData;

impl SweepBuffer {
    pub fn new() -> SweepBuffer {
        SweepBuffer {
            index: AtomicUsize::new(0),
            spine_length: AtomicUsize::new(0),
            spine_cap: Mutex::new(0),
            spine: Default::default(),
        }
    }
}

pub struct PushableSweepBuffer(pub SweepBuffer);
pub struct PopableSweepBuffer(pub SweepBuffer);

impl PopableSweepBuffer {
    // pop removes and returns a span from buffer b, or nil if b is empty.
    // pop is safe to call concurrently with other pop operations, but NOT
    // to call concurrently with push.
    pub fn pop(&self) -> Option<MemorySpan> {
    	let cursor = self.0.index.fetch_sub(1, Ordering::Relaxed);
    	if cursor == 0 {
            self.0.index.fetch_add(1, Ordering::Relaxed);
            None
    	} else {
            let cursor = cursor - 1; // fetch_sub returns old cursor
            // There are no concurrent spine or block modifications during
            // pop, so we can omit the atomics.
            // TODO Unfortunately it's unstable to read atomics unatomically in Rust do this later?
            let (top, bottom) = (cursor / GC_SWEEP_BLOCK_ENTRIES, cursor % GC_SWEEP_BLOCK_ENTRIES);
            // blockp := (**gcSweepBlock)(b.spine + sys.PtrSize*uintptr(top)))
            // block := *blockp
            // s := block.spans[bottom]
            // // Clear the pointer for block(i).
            // block.spans[bottom] = nil
            // return s
            None
        }
    }
}

impl PushableSweepBuffer {
    // push adds span s to buffer b. push is safe to call concurrently
    // with other push operations, but NOT to call concurrently with pop.
    pub fn push(&self, span: MemorySpan) {
        // Obtain our slot.
        let cursor = self.0.index.fetch_add(1, Ordering::Relaxed);
        let (top, bottom) = (cursor / GC_SWEEP_BLOCK_ENTRIES, cursor % GC_SWEEP_BLOCK_ENTRIES);

    	// Do we need to add a block?
        let mut spine_length = self.0.spine_length.load(Ordering::Relaxed);
        let block : SweepBlock = loop {
            if top < spine_length {
                // 		spine := atomic.Loadp(unsafe.Pointer(&b.spine))
                // 		blockp := add(spine, sys.PtrSize*top)
                // 		block = (*gcSweepBlock)(atomic.Loadp(blockp))
            } else {
            	// Add a new block to the spine, potentially growing
                // the spine.
                let mut spine_cap = *self.0.spine_cap.lock().expect("Could not unlock spine cap");
         		// spine_length cannot change until we release the lock,
         		// but may have changed while we were waiting.
                spine_length = self.0.spine_length.load(Ordering::Relaxed);
        		if top < spine_length {
        			continue
        		} else {
                    if spine_length == spine_cap {
            			// Grow the spine.
                        let new_cap = if spine_cap != 0 {
                            spine_cap * 2
                        } else {
                        	GC_SWEEP_BUF_INIT_SPINE_CAP
                        };

                        let layout = Layout::array::<SweepBlock>(new_cap).expect("Could not layout spine");

                        // TODO: Blocks are allocated off-heap, so no write barriers.
                        // TODO: new_spine = persistent_alloc(new_cap*sys.PtrSize, sys.CacheLineSize,
                        // &memstats.gc_sys)
                        let mut new_spine = unsafe {
                            let mut spine = mem::transmute::<*mut SweepBlock, *mut u8>(self.0.spine.load(Ordering::Relaxed));
                            mem::transmute::<*mut u8, *mut SweepBlock>(
                                if spine_cap == 0 {
                                    alloc(layout)
                                } else {
                                    realloc(spine, layout, spine_cap)
                                }
                            )
                        };

                        // TODO: Spine is allocated off-heap, so no write barrier.
                        self.0.spine.store(new_spine, Ordering::Relaxed);
                        spine_cap = new_cap;
            			// We can't immediately free the old spine
            			// since a concurrent push with a lower index
            			// could still be reading from it. We let it
            			// leak because even a 1TB heap would waste
            			// less than 2MB of memory on old spines. If
            			// this is a problem, we could free old spines
            			// during STW.
            		}

            		// Allocate a new block and add it to the spine.
            		unsafe {
                        let block = mem::transmute::<*mut u8, SweepBlock>(alloc(Layout::new::<SweepBlockData>()));
                        let block_ptr = self.0.spine.load(Ordering::Relaxed).offset(top as isize);
                		// Blocks are allocated off-heap, so no write barrier.
                        let block_ptr_atomic : AtomicPtr<SweepBlockData> = mem::transmute(block_ptr);
                        block_ptr_atomic.store(block, Ordering::Relaxed);
                        self.0.spine_length.fetch_add(1, Ordering::Relaxed);
                    };
                }
            }
        };
    	// We have a block. Insert the span.
        block.spans[bottom] = span;
    }
}
