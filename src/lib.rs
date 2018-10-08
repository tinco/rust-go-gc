// Copyright 2009 The Go Authors. All rights reserved.
// Use of this source code is governed by a BSD-style
// license that can be found in the LICENSE file.
// Copyright 2018 Tinco Andringa.

#![feature(async_await)]
#![feature(futures_api)]
#![feature(await_macro)]
#![feature(pin)]
#![feature(arbitrary_self_types)]
#![feature(generators)]
#![feature(allocator_api)]
#![feature(ptr_internals)]
#![feature(int_to_from_bytes)]

pub mod gc;
pub mod memory_heap;
pub mod memory_span;
pub mod size_classes;
pub mod sweep_buffer;

#[test]
fn it_works() {}
