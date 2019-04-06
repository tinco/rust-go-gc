use memmap::MmapMut;
use std::io::Error;
use std::marker::PhantomData;


pub struct StaticVec<T> {
    buf: MmapMut,
    len: usize,
    phantom: PhantomData<T>,
}

impl<T> StaticVec<T> {
    pub const fn new(length: usize) -> Result<StaticVec<T>, Error> {
        let maybe_buf = MmapMut::map_anon(0);
        match maybe_buf {
            Ok(buf) => Ok(StaticVec {
                buf: buf,
                len: 0,
                phantom: PhantomData,
            }),
            Err(e) => Err(e)
        }
    }
}
