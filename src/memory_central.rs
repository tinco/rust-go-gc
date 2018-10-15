use super::memory_span::*;

pub struct MemoryCentral {}
impl MemoryCentral {
    pub fn new() -> MemoryCentral {
        MemoryCentral {}
    }

    pub fn free_span(&self, span: MemorySpan, preserve: bool, was_empty: bool) -> bool {
        false
    }
}
