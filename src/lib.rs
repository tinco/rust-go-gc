// Copyright 2009 The Go Authors. All rights reserved.
// Use of this source code is governed by a BSD-style
// license that can be found in the LICENSE file.
// Copyright 2018 Tinco Andringa.

#![feature(async_await)]
#![feature(futures_api)]
#![feature(await_macro)]
#![feature(arbitrary_self_types)]
#![feature(generators)]
#![feature(allocator_api)]
#![feature(ptr_internals)]
#![feature(alloc_layout_extra)]

pub mod gc;
pub mod memory_central;
pub mod memory_heap;
pub mod memory_span;
pub mod memory_span_list;
pub mod size_classes;
pub mod sweep_buffer;
pub mod static_vec;
pub mod work;
pub mod memory_allocator;

#[test]
fn it_works() {}
