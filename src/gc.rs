use back_to_the_future::futures_await;
use futures::stream::Stream;
use futures::sync::{mpsc, oneshot};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use super::memory_heap::*;
use super::memory_span;
use super::work::*;

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
        let mut sender_field = self
            .unpark_sender
            .lock()
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

const GC_BITS_CHUNK_BYTES: usize = 64 << 10;
// const GC_BITS_HEADER_BYTES : usize = unsafe.Sizeof(gcBitsHeader{});

struct GCBitsHeader {
    // free uintptr // free is the index into bits of the next free byte.
// next uintptr // *gcBits triggers recursive type bug. (issue 14620)
}

//go:notinheap
struct GCBitsArena {
    // // gcBitsHeader // side step recursive type bug (issue 14620) by including fields by hand.
// free uintptr // free is the index into bits of the next free byte; read/write atomically
// next *gcBitsArena
// bits [gcBitsChunkBytes - gcBitsHeaderBytes]gcBits
}

struct GCBitsArenas {
    // lock     mutex
// free     *gcBitsArena
// next     *gcBitsArena // Read atomically. Write atomically under lock.
// current  *gcBitsArena
// previous *gcBitsArena
}

#[repr(usize)]
pub enum GCPhase {
    Off,
}

pub struct GC {
    // Garbage collector phase.
    // Indicates to write barrier and synchronization task to perform.
    phase: AtomicUsize,
    background_sweep: BackgroundSweep,
    pub memory_heap: Box<MemoryHeap>,
    bits_arenas: GCBitsArenas,
    work: Work,
}

pub fn new_gc() -> GC {
    GC {
        phase: AtomicUsize::new(GCPhase::Off as usize),
        background_sweep: new_sweep(),
        memory_heap: MemoryHeap::new(),
        bits_arenas: GCBitsArenas {},
        work: Work::new(),
    }
}

impl GC {
    // gcenable is called after the bulk of the runtime initialization,
    // just before we're about to start letting user code run.
    // It kicks off the background sweeper routine and enables GC.
    pub async fn enable(&mut self) {
        let (signal_setup_done, mut setup_done) = oneshot::channel::<bool>();

        self.background_sweep(signal_setup_done);

        futures_await!(&mut setup_done);
    }

    async fn background_sweep(&mut self, signal_setup_done: oneshot::Sender<bool>) {
        // This assignment is to a block, so that the lock is released at the end of it and
        // we can safely wait for something to signal us that the gc is ready.
        let unpark = {
            let mut sweep_parked = self
                .background_sweep
                .parked
                .lock()
                .expect("Could not get lock on SweepData");
            *sweep_parked = true;

            let _ = signal_setup_done.send(true);

            self.background_sweep.wait_for_unpark()
        };

        let mut unpark_future = unpark.into_future();
        futures_await!(&mut unpark_future);

        loop {
            // we sweep one and then yield to other goroutines until sweepone returns FFFFFFF
            while self.sweep_one() != !0 {
                self.background_sweep.number.fetch_add(1, Ordering::Relaxed);
                yield
            }

            while self.free_some_work_buffers(true) {
                yield
            }

            let mut sweep_parked = self
                .background_sweep
                .parked
                .lock()
                .expect("Could not get background sweeper parked lock");

            if !self.memory_heap.sweep_done.load(Ordering::Relaxed) {
                // This can happen if a GC runs between
                // sweep_one returning !0 above
                // and the lock being acquired.
                continue;
            }

            *sweep_parked = true;
            futures_await!(&mut unpark_future);
        }
    }

    // freeSomeWbufs frees some workbufs back to the heap and returns
    // true if it should be called again to free more.
    fn free_some_work_buffers(&mut self, preemptible: bool) -> bool {
        let batch_size = 64; // ~1–2 µs per span.
        let work_buffer_spans = self
            .work
            .work_buffer_spans
            .protected
            .lock()
            .expect("Could not lock work buffer spans");

        if self.phase.load(Ordering::Relaxed) != (GCPhase::Off as usize)
            || work_buffer_spans.free.is_empty()
        {
            return false;
        }

        // 	gp := getg().m.curg
        for _ in 0..batch_size {
            // TODO do we support preemption?
            // if (preemptible && gp.preempt) {
            //     break
            // }
            match work_buffer_spans.free.remove_first() {
                Some(span) => {
                    // mheap_.freeManual(span, &memstats.gc_sys)
                    let memory_heap: &mut MemoryHeap = self.memory_heap.as_mut();
                    memory_heap.free_manual_span(span)
                }
                None => break,
            }
        }

        return !work_buffer_spans.free.is_empty();
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
        if self.memory_heap.sweep_done.load(Ordering::Relaxed) {
            !0
        } else {
            self.memory_heap.sweepers.fetch_add(1, Ordering::Relaxed);
            let mut number_of_pages: usize = !0;
            let sweep_generation = self.memory_heap.sweep_generation.load(Ordering::Relaxed);

            loop {
                let maybe_span = self
                    .memory_heap
                    .get_popable_sweep_buffer(sweep_generation)
                    .pop();
                match maybe_span {
                    None => {
                        self.memory_heap.sweep_done.store(true, Ordering::Relaxed);
                    }
                    Some(mut span) => {
                        let span = unsafe { span.as_mut() };
                        //TODO non-atomic read on sweep_generation?
                        let span_sweep_generation = span.sweep_generation.load(Ordering::Relaxed);
                        if span.state != memory_span::State::InUse {
                            // This can happen if direct sweeping already
                            // swept this span, but in that case the sweep
                            // generation should always be up-to-date.
                            if span_sweep_generation != sweep_generation {
                                // print("runtime: bad span s.state=", s.state, " s.sweepgen=", s.sweepgen, " sweepgen=", sg, "\n")
                                panic!("non in-use span in unswept list")
                            }
                            continue;
                        }
                        if span_sweep_generation != sweep_generation - 2 ||
                        // if the span_sweep_generation is not sweep_generation - 2
                        // or, if it is, but when we try to update it to be sweep_generation -1
                        //, it is not, we continue the loop
                        sweep_generation - 2 == span.sweep_generation.compare_and_swap(
                                sweep_generation - 2, sweep_generation - 1, Ordering::Relaxed)
                        {
                            continue;
                        }
                        number_of_pages = span.number_of_pages;
                        if !span.sweep(self, false) {
                            // Span is still in-use, so this returned no
                            // pages to the heap and the span needs to
                            // move to the swept in-use list.
                            number_of_pages = 0;
                        }
                    }
                };
                break;
            }

            // Decrement the number of active sweepers and if this is the
            // last one print trace information.
            if self.memory_heap.sweepers.fetch_sub(1, Ordering::Relaxed) == 0
                && self.memory_heap.sweep_done.load(Ordering::Relaxed)
            {
                // if debug.gcpacertrace > 0 {
                // 	print("pacer: sweep done at heap size ", memstats.heap_live>>20, "MB; allocated ", (memstats.heap_live-mheap_.sweepHeapLiveBasis)>>20, "MB during sweep; swept ", mheap_.pagesSwept, " pages at ", sweepRatio, " pages/byte\n")
                // }
            }

            number_of_pages
        }
    }

    pub fn new_mark_bits(&mut self, number_of_elements: usize) -> std::ptr::Unique<u8> {
        std::ptr::Unique::empty()
    }
}
