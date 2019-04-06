use memmap::{MmapOptions, MmapMut};
use std::io::Error;
use std::marker::PhantomData;
use super::memory_allocator::*;

///
/// Never is reallocated, grows contiguously in multitudes of memory pages.
///
pub struct StaticVec<T> {
    buffer: MmapMut,
    capacity: usize,
    length: usize,
    phantom: PhantomData<T>,
}

impl<T> StaticVec<T> {
    pub fn new(minimum_capacity: usize) -> Result<StaticVec<T>, Error> {
        let minimum_size = minimum_capacity * std::mem::size_of::<T>();
        let size = minimum_size / PAGE_SIZE;
        let maybe_buf = MmapOptions::new().len(0).map_anon();
        match maybe_buf {
            Ok(buf) => Ok(StaticVec {
                buffer: buf,
                length: 0,
                capacity: 0,
                phantom: PhantomData,
            }),
            Err(e) => Err(e)
        }
    }
}

// // round n up to a multiple of a.  a must be a power of 2.
// func round(n, a uintptr) uintptr {
// 	return (n + a - 1) &^ (a - 1)
// }
