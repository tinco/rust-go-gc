use super::memory_allocator::*;
use memmap::{MmapMut, MmapOptions};
use std::io::Error;
use std::marker::PhantomData;

///
/// Never is reallocated, grows contiguously in multitudes of memory pages.
///

// linearAlloc is a simple linear allocator that pre-reserves a region
// of memory and then maps that region as needed. The caller is
// responsible for locking.
// type linearAlloc struct {
// 	next   uintptr // next free byte
// 	mapped uintptr // one byte past end of mapped space
// 	end    uintptr // end of reserved space
// }
//
// func (l *linearAlloc) init(base, size uintptr) {
// 	l.next, l.mapped = base, base
// 	l.end = base + size
// }
//
// func (l *linearAlloc) alloc(size, align uintptr, sysStat *uint64) unsafe.Pointer {
// 	p := round(l.next, align)
// 	if p+size > l.end {
// 		return nil
// 	}
// 	l.next = p + size
// 	if pEnd := round(l.next-1, physPageSize); pEnd > l.mapped {
// 		// We need to map more of the reserved space.
// 		sysMap(unsafe.Pointer(l.mapped), pEnd-l.mapped, sysStat)
// 		l.mapped = pEnd
// 	}
// 	return unsafe.Pointer(p)
// }

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
            Err(e) => Err(e),
        }
    }
}

// // round n up to a multiple of a.  a must be a power of 2.
// func round(n, a uintptr) uintptr {
// 	return (n + a - 1) &^ (a - 1)
// }
