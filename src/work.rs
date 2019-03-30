use std::sync::Mutex;
use crate::memory_span_list::MemorySpanList;

// 	wbufSpans struct {
pub struct WorkBufferSpansProtected {
	// free is a list of spans dedicated to workbufs, but
	// that don't currently contain any workbufs.
	pub free: MemorySpanList,
	// busy is a list of all spans containing workbufs on
	// one of the workbuf lists.
	pub busy: MemorySpanList,
}

impl WorkBufferSpansProtected {
    pub fn new() -> WorkBufferSpansProtected {
        WorkBufferSpansProtected {
            free: MemorySpanList{},
            busy: MemorySpanList{},
        }
    }
}

pub struct WorkBufferSpans {
    pub protected: Mutex<WorkBufferSpansProtected>,
}

impl WorkBufferSpans {
    pub fn new() -> WorkBufferSpans {
        WorkBufferSpans {
            protected: Mutex::new(WorkBufferSpansProtected::new())
        }
    }
}

pub struct Work {
    // 	full  lfstack                  // lock-free list of full blocks workbuf
    // 	empty lfstack                  // lock-free list of empty blocks workbuf
    // 	pad0  [sys.CacheLineSize]uint8 // prevents false-sharing between full/empty and nproc/nwait
    //
    pub work_buffer_spans: WorkBufferSpans,
    //
    // 	// Restore 64-bit alignment on 32-bit.
    // 	_ uint32
    //
    // 	// bytesMarked is the number of bytes marked this cycle. This
    // 	// includes bytes blackened in scanned objects, noscan objects
    // 	// that go straight to black, and permagrey objects scanned by
    // 	// markroot during the concurrent scan phase. This is updated
    // 	// atomically during the cycle. Updates may be batched
    // 	// arbitrarily, since the value is only read at the end of the
    // 	// cycle.
    // 	//
    // 	// Because of benign races during marking, this number may not
    // 	// be the exact number of marked bytes, but it should be very
    // 	// close.
    // 	//
    // 	// Put this field here because it needs 64-bit atomic access
    // 	// (and thus 8-byte alignment even on 32-bit architectures).
    // 	bytesMarked uint64
    //
    // 	markrootNext uint32 // next markroot job
    // 	markrootJobs uint32 // number of markroot jobs
    //
    // 	nproc   uint32
    // 	tstart  int64
    // 	nwait   uint32
    // 	ndone   uint32
    // 	alldone note
    //
    // 	// helperDrainBlock indicates that GC mark termination helpers
    // 	// should pass gcDrainBlock to gcDrain to block in the
    // 	// getfull() barrier. Otherwise, they should pass gcDrainNoBlock.
    // 	//
    // 	// TODO: This is a temporary fallback to work around races
    // 	// that cause early mark termination.
    // 	helperDrainBlock bool
    //
    // 	// Number of roots of various root types. Set by gcMarkRootPrepare.
    // 	nFlushCacheRoots                               int
    // 	nDataRoots, nBSSRoots, nSpanRoots, nStackRoots int
    //
    // 	// markrootDone indicates that roots have been marked at least
    // 	// once during the current GC cycle. This is checked by root
    // 	// marking operations that have to happen only during the
    // 	// first root marking pass, whether that's during the
    // 	// concurrent mark phase in current GC or mark termination in
    // 	// STW GC.
    // 	markrootDone bool
    //
    // 	// Each type of GC state transition is protected by a lock.
    // 	// Since multiple threads can simultaneously detect the state
    // 	// transition condition, any thread that detects a transition
    // 	// condition must acquire the appropriate transition lock,
    // 	// re-check the transition condition and return if it no
    // 	// longer holds or perform the transition if it does.
    // 	// Likewise, any transition must invalidate the transition
    // 	// condition before releasing the lock. This ensures that each
    // 	// transition is performed by exactly one thread and threads
    // 	// that need the transition to happen block until it has
    // 	// happened.
    // 	//
    // 	// startSema protects the transition from "off" to mark or
    // 	// mark termination.
    // 	startSema uint32
    // 	// markDoneSema protects transitions from mark 1 to mark 2 and
    // 	// from mark 2 to mark termination.
    // 	markDoneSema uint32
    //
    // 	bgMarkReady note   // signal background mark worker has started
    // 	bgMarkDone  uint32 // cas to 1 when at a background mark completion point
    // 	// Background mark completion signaling
    //
    // 	// mode is the concurrency mode of the current GC cycle.
    // 	mode gcMode
    //
    // 	// userForced indicates the current GC cycle was forced by an
    // 	// explicit user call.
    // 	userForced bool
    //
    // 	// totaltime is the CPU nanoseconds spent in GC since the
    // 	// program started if debug.gctrace > 0.
    // 	totaltime int64
    //
    // 	// initialHeapLive is the value of memstats.heap_live at the
    // 	// beginning of this GC cycle.
    // 	initialHeapLive uint64
    //
    // 	// assistQueue is a queue of assists that are blocked because
    // 	// there was neither enough credit to steal or enough work to
    // 	// do.
    // 	assistQueue struct {
    // 		lock       mutex
    // 		head, tail guintptr
    // 	}
    //
    // 	// sweepWaiters is a list of blocked goroutines to wake when
    // 	// we transition from mark termination to sweep.
    // 	sweepWaiters struct {
    // 		lock mutex
    // 		head guintptr
    // 	}
    //
    // 	// cycles is the number of completed GC cycles, where a GC
    // 	// cycle is sweep termination, mark, mark termination, and
    // 	// sweep. This differs from memstats.numgc, which is
    // 	// incremented at mark termination.
    // 	cycles uint32
    //
    // 	// Timing/utilization stats for this cycle.
    // 	stwprocs, maxprocs                 int32
    // 	tSweepTerm, tMark, tMarkTerm, tEnd int64 // nanotime() of phase start
    //
    // 	pauseNS    int64 // total STW time this cycle
    // 	pauseStart int64 // nanotime() of last STW
    //
    // 	// debug.gctrace heap sizes for this cycle.
    // 	heap0, heap1, heap2, heapGoal uint64
}
impl Work {
    pub fn new() -> Work {
        Work {
            work_buffer_spans: WorkBufferSpans::new()
        }
    }
}

// var work struct {

// }
