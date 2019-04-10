use super::size_classes::*;

pub const POINTER_SIZE: usize = std::mem::size_of::<&u8>();
pub const IS_64_BIT: bool = POINTER_SIZE == 8;

#[cfg(target_os = "windows")]
pub const OS_IS_WINDOWS: bool = true;
#[cfg(not(target_os = "windows"))]
pub const OS_IS_WINDOWS: bool = false;

#[cfg(target_os = "aix")]
pub const OS_IS_AIX: bool = true;
#[cfg(not(target_os = "aix"))]
pub const OS_IS_AIX: bool = false;

// Copyright 2014 The Go Authors. All rights reserved.
// Use of this source code is governed by a BSD-style
// license that can be found in the LICENSE file.

// Memory allocator.
//
// This was originally based on tcmalloc, but has diverged quite a bit.
// http://goog-perftools.sourceforge.net/doc/tcmalloc.html

// The main allocator works in runs of pages.
// Small allocation sizes (up to and including 32 kB) are
// rounded to one of about 70 size classes, each of which
// has its own free set of objects of exactly that size.
// Any free page of memory can be split into a set of objects
// of one size class, which are then managed using a free bitmap.
//
// The allocator's data structures are:
//
//	fixalloc: a free-list allocator for fixed-size off-heap objects,
//		used to manage storage used by the allocator.
//	mheap: the malloc heap, managed at page (8192-byte) granularity.
//	mspan: a run of pages managed by the mheap.
//	mcentral: collects all spans of a given size class.
//	mcache: a per-P cache of mspans with free space.
//	mstats: allocation statistics.
//
// Allocating a small object proceeds up a hierarchy of caches:
//
//	1. Round the size up to one of the small size classes
//	   and look in the corresponding mspan in this P's mcache.
//	   Scan the mspan's free bitmap to find a free slot.
//	   If there is a free slot, allocate it.
//	   This can all be done without acquiring a lock.
//
//	2. If the mspan has no free slots, obtain a new mspan
//	   from the mcentral's list of mspans of the required size
//	   class that have free space.
//	   Obtaining a whole span amortizes the cost of locking
//	   the mcentral.
//
//	3. If the mcentral's mspan list is empty, obtain a run
//	   of pages from the mheap to use for the mspan.
//
//	4. If the mheap is empty or has no page runs large enough,
//	   allocate a new group of pages (at least 1MB) from the
//	   operating system. Allocating a large run of pages
//	   amortizes the cost of talking to the operating system.
//
// Sweeping an mspan and freeing objects on it proceeds up a similar
// hierarchy:
//
//	1. If the mspan is being swept in response to allocation, it
//	   is returned to the mcache to satisfy the allocation.
//
//	2. Otherwise, if the mspan still has allocated objects in it,
//	   it is placed on the mcentral free list for the mspan's size
//	   class.
//
//	3. Otherwise, if all objects in the mspan are free, the mspan
//	   is now "idle", so it is returned to the mheap and no longer
//	   has a size class.
//	   This may coalesce it with adjacent idle mspans.
//
//	4. If an mspan remains idle for long enough, return its pages
//	   to the operating system.
//
// Allocating and freeing a large object uses the mheap
// directly, bypassing the mcache and mcentral.
//
// Free object slots in an mspan are zeroed only if mspan.needzero is
// false. If needzero is true, objects are zeroed as they are
// allocated. There are various benefits to delaying zeroing this way:
//
//	1. Stack frame allocation can avoid zeroing altogether.
//
//	2. It exhibits better temporal locality, since the program is
//	   probably about to write to the memory.
//
//	3. We don't zero pages that never get reused.
//
// const (
// 	debugMalloc = false
//
// 	maxTinySize   = _TinySize
// 	tinySizeClass = _TinySizeClass
// 	maxSmallSize  = _MaxSmallSize
//
// 	pageShift = _PageShift
// 	pageSize  = _PageSize
// 	pageMask  = _PageMask
// 	// By construction, single page spans of the smallest object class
// 	// have the most objects per span.
// 	maxObjsPerSpan = pageSize / 8
//
// 	mSpanInUse = _MSpanInUse
//
// 	concurrentSweep = _ConcurrentSweep
//
pub const PAGE_SIZE: usize = 1 << PAGE_SHIFT;
// 	_PageMask = _PageSize - 1
//
// 	// _64bit = 1 on 64-bit systems, 0 on 32-bit systems
// 	_64bit = 1 << (^uintptr(0) >> 63) / 2
//
// 	// Tiny allocator parameters, see "Tiny allocator" comment in malloc.go.
// 	_TinySize      = 16
// 	_TinySizeClass = int8(2)
//
// 	_FixAllocChunk  = 16 << 10               // Chunk size for FixAlloc
// 	_MaxMHeapList   = 1 << (20 - _PageShift) // Maximum page length for fixed-size list in MHeap.
// 	_HeapAllocChunk = 1 << 20                // Chunk size for heap growth
//
// 	// Per-P, per order stack segment cache size.
// 	_StackCacheSize = 32 * 1024
//
// 	// Number of orders that get caching. Order 0 is FixedStack
// 	// and each successive order is twice as large.
// 	// We want to cache 2KB, 4KB, 8KB, and 16KB stacks. Larger stacks
// 	// will be allocated directly.
// 	// Since FixedStack is different on different systems, we
// 	// must vary NumStackOrders to keep the same maximum cached size.
// 	//   OS               | FixedStack | NumStackOrders
// 	//   -----------------+------------+---------------
// 	//   linux/darwin/bsd | 2KB        | 4
// 	//   windows/32       | 4KB        | 3
// 	//   windows/64       | 8KB        | 2
// 	//   plan9            | 4KB        | 3
// 	_NumStackOrders = 4 - sys.PtrSize/4*sys.GoosWindows - 1*sys.GoosPlan9
//
// 	// Number of bits in page to span calculations (4k pages).
// 	// On Windows 64-bit we limit the arena to 32GB or 35 bits.
// 	// Windows counts memory used by page table into committed memory
// 	// of the process, so we can't reserve too much memory.
// 	// See https://golang.org/issue/5402 and https://golang.org/issue/5236.
// 	// On other 64-bit platforms, we limit the arena to 512GB, or 39 bits.
// 	// On 32-bit, we don't bother limiting anything, so we use the full 32-bit address.
// 	// The only exception is mips32 which only has access to low 2GB of virtual memory.
// 	// On Darwin/arm64, we cannot reserve more than ~5GB of virtual memory,
// 	// but as most devices have less than 4GB of physical memory anyway, we
// 	// try to be conservative here, and only ask for a 2GB heap.
// 	_MHeapMap_TotalBits = (_64bit*sys.GoosWindows)*35 + (_64bit*(1-sys.GoosWindows)*(1-sys.GoosDarwin*sys.GoarchArm64))*39 + sys.GoosDarwin*sys.GoarchArm64*31 + (1-_64bit)*(32-(sys.GoarchMips+sys.GoarchMipsle))
// 	_MHeapMap_Bits      = _MHeapMap_TotalBits - _PageShift
//
// 	// _MaxMem is the maximum heap arena size minus 1.
// 	//
// 	// On 32-bit, this is also the maximum heap pointer value,
// 	// since the arena starts at address 0.
// 	_MaxMem = 1<<_MHeapMap_TotalBits - 1
//
// 	// Max number of threads to run garbage collection.
// 	// 2, 3, and 4 are all plausible maximums depending
// 	// on the hardware details of the machine. The garbage
// 	// collector scales well to 32 cpus.
// 	_MaxGcproc = 32
//
// 	// minLegalPointer is the smallest possible legal pointer.
// 	// This is the smallest possible architectural page size,
// 	// since we assume that the first page is never mapped.
// 	//
// 	// This should agree with minZeroPage in the compiler.
// 	minLegalPointer uintptr = 4096

// heapAddrBits is the number of bits in a heap address. On
// amd64, addresses are sign-extended beyond heapAddrBits. On
// other arches, they are zero-extended.
//
// On most 64-bit platforms, we limit this to 48 bits based on a
// combination of hardware and OS limitations.
//
// amd64 hardware limits addresses to 48 bits, sign-extended
// to 64 bits. Addresses where the top 16 bits are not either
// all 0 or all 1 are "non-canonical" and invalid. Because of
// these "negative" addresses, we offset addresses by 1<<47
// (arenaBaseOffset) on amd64 before computing indexes into
// the heap arenas index. In 2017, amd64 hardware added
// support for 57 bit addresses; however, currently only Linux
// supports this extension and the kernel will never choose an
// address above 1<<47 unless mmap is called with a hint
// address above 1<<47 (which we never do).
//
// arm64 hardware (as of ARMv8) limits user addresses to 48
// bits, in the range [0, 1<<48).
//
// ppc64, mips64, and s390x support arbitrary 64 bit addresses
// in hardware. On Linux, Go leans on stricter OS limits. Based
// on Linux's processor.h, the user address space is limited as
// follows on 64-bit architectures:
//
// Architecture  Name              Maximum Value (exclusive)
// ---------------------------------------------------------------------
// amd64         TASK_SIZE_MAX     0x007ffffffff000 (47 bit addresses)
// arm64         TASK_SIZE_64      0x01000000000000 (48 bit addresses)
// ppc64{,le}    TASK_SIZE_USER64  0x00400000000000 (46 bit addresses)
// mips64{,le}   TASK_SIZE64       0x00010000000000 (40 bit addresses)
// s390x         TASK_SIZE         1<<64 (64 bit addresses)
//
// These limits may increase over time, but are currently at
// most 48 bits except on s390x. On all architectures, Linux
// starts placing mmap'd regions at addresses that are
// significantly below 48 bits, so even if it's possible to
// exceed Go's 48 bit limit, it's extremely unlikely in
// practice.
//
// On aix/ppc64, the limits is increased to 1<<60 to accept addresses
// returned by mmap syscall. These are in range:
//  0x0a00000000000000 - 0x0afffffffffffff
//
// On 32-bit platforms, we accept the full 32-bit address
// space because doing so is cheap.
// mips32 only has access to the low 2GB of virtual memory, so
// we further limit it to 31 bits.
//
// WebAssembly currently has a limit of 4GB linear memory.
#[cfg(all(
    target_pointer_width = "64",
    not(target_arch = "wasm32"),
    not(target_os = "aix")
))]
pub const HEAP_ADDRESS_BITS: usize = 48;

#[cfg(all(
    any(not(target_pointer_width = "64"), target_arch = "wasm32"),
    not(target_arch = "mips")
))]
pub const HEAP_ADDRESS_BITS: usize = 32;

#[cfg(all(target_arch = "mips", not(target_pointer_width = "64")))]
pub const HEAP_ADDRESS_BITS: usize = 31;

#[cfg(target_arch = "aix")]
pub const HEAP_ADDRESS_BITS: usize = 60;

// maxAlloc is the maximum size of an allocation. On 64-bit,
// it's theoretically possible to allocate 1<<heapAddrBits bytes. On
// 32-bit, however, this is one less than 1<<32 because the
// number of bytes in the address space doesn't actually fit
// in a uintptr.
#[cfg(target_pointer_width = "64")]
pub const MAX_ALLOCATION_SIZE: usize = 1 << HEAP_ADDRESS_BITS;
#[cfg(target_pointer_width = "32")]
pub const MAX_ALLOCATION_SIZE: usize = (1 << HEAP_ADDRESS_BITS) - 1;

// The number of bits in a heap address, the size of heap
// arenas, and the L1 and L2 arena map sizes are related by
//
//   (1 << addr bits) = arena size * L1 entries * L2 entries
//
// Currently, we balance these as follows:
//
//       Platform  Addr bits  Arena size  L1 entries   L2 entries
// --------------  ---------  ----------  ----------  -----------
//       */64-bit         48        64MB           1    4M (32MB)
//     aix/64-bit         60       256MB        4096    4M (32MB)
// windows/64-bit         48         4MB          64    1M  (8MB)
//       */32-bit         32         4MB           1  1024  (4KB)
//     */mips(le)         31         4MB           1   512  (2KB)

// heapArenaBytes is the size of a heap arena. The heap
// consists of mappings of size heapArenaBytes, aligned to
// heapArenaBytes. The initial heap mapping is one arena.
//
// This is currently 64MB on 64-bit non-Windows and 4MB on
// 32-bit and on Windows. We use smaller arenas on Windows
// because all committed memory is charged to the process,
// even if it's not touched. Hence, for processes with small
// heaps, the mapped arena space needs to be commensurate.
// This is particularly important with the race detector,
// since it significantly amplifies the cost of committed
// memory.
pub const HEAP_ARENA_BYTES: usize = 1 << LOG_HEAP_ARENA_BYTES;

// logHeapArenaBytes is log_2 of heapArenaBytes. For clarity,
// prefer using heapArenaBytes where possible (we need the
// constant to compute some other constants).

// pub const LOG_HEAP_ARENA_BYTES: usize =
//     (6+20)*((IS_64_BIT as usize) * (1-(OS_IS_WINDOWS as usize))*(1-OS_IS_AIX as usize)) +
//     (2+20)*((IS_64_BIT as usize) * (OS_IS_WINDOWS as usize)) +
//     (2+20)*(1-(IS_64_BIT as usize)) +
//     (8+20)*(OS_IS_AIX as usize);

#[cfg(all(target_pointer_width = "64", target_os = "windows"))]
pub const LOG_HEAP_ARENA_BYTES: usize = 2 + 20;

#[cfg(all(target_pointer_width = "64", target_os = "aix"))]
pub const LOG_HEAP_ARENA_BYTES: usize = 8 + 20;

#[cfg(all(
    target_pointer_width = "64",
    not(all(target_os = "aix", target_os = "windows"))
))]
pub const LOG_HEAP_ARENA_BYTES: usize = 6 + 20;

#[cfg(target_pointer_width = "32")]
pub const LOG_HEAP_ARENA_BYTES: usize = 0;

// heapArenaBitmapBytes is the size of each heap arena's bitmap.
pub const HEAP_ARENA_BITMAP_BYTES: usize = HEAP_ARENA_BYTES / (POINTER_SIZE * 8 / 2);

pub const PAGES_PER_ARENA: usize = HEAP_ARENA_BYTES / PAGE_SIZE;

// arenaL1Bits is the number of bits of the arena number
// covered by the first level arena map.
//
// This number should be small, since the first level arena
// map requires PtrSize*(1<<arenaL1Bits) of space in the
// binary's BSS. It can be zero, in which case the first level
// index is effectively unused. There is a performance benefit
// to this, since the generated code can be more efficient,
// but comes at the cost of having a large L2 mapping.
//
// We use the L1 map on 64-bit Windows because the arena size
// is small, but the address space is still 48 bits, and
// there's a high cost to having a large L2.
//
// We use the L1 map on aix/ppc64 to keep the same L2 value
// as on Linux.

#[cfg(all(target_pointer_width = "64", target_os = "windows"))]
pub const ARENA_LEVEL_1_BITS: usize = 6;

#[cfg(target_os = "aix")]
pub const ARENA_LEVEL_1_BITS: usize = 12;

#[cfg(all(
    not(target_os = "aix"),
    not(all(target_os = "windows", target_pointer_width = "64"))
))]
pub const ARENA_LEVEL_1_BITS: usize = 0;

// arenaL2Bits is the number of bits of the arena number
// covered by the second level arena index.
//
// The size of each arena map allocation is proportional to
// 1<<arenaL2Bits, so it's important that this not be too
// large. 48 bits leads to 32MB arena index allocations, which
// is about the practical threshold.
pub const ARENA_LEVEL_2_BITS: usize = HEAP_ADDRESS_BITS - LOG_HEAP_ARENA_BYTES - ARENA_LEVEL_1_BITS;

// arenaL1Shift is the number of bits to shift an arena frame
// number by to compute an index into the first level arena map.
pub const ARENA_LEVEL_1_SHIFT: usize = ARENA_LEVEL_2_BITS;

// arenaBits is the total bits in a combined arena map index.
// This is split between the index into the L1 arena map and
// the L2 arena map.
pub const ARENA_BITS: usize = ARENA_LEVEL_1_BITS + ARENA_LEVEL_2_BITS;

// arenaBaseOffset is the pointer value that corresponds to
// index 0 in the heap arena map.
//
// On amd64, the address space is 48 bits, sign extended to 64
// bits. This offset lets us handle "negative" addresses (or
// high addresses if viewed as unsigned).
//
// On other platforms, the user address space is contiguous
// and starts at 0, so no offset is necessary.
#[cfg(target_pointer_width = "64")]
pub const ARENA_BASE_OFFSET: usize = 1 << 47;
#[cfg(target_pointer_width = "32")]
pub const ARENA_BASE_OFFSET: usize = 0;

// )
