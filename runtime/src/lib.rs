use libc::size_t;
use rand::Rng;
use std::mem::swap;

use crate::cgt::on_strike;

pub mod cgt;

const NOT_MARKABLE: usize = 768;
const CUSTOM_TAG: u64 = 255;

#[derive(Copy, Clone)]
#[repr(C)]
pub struct Value(u64);

pub struct Pool;

#[derive(Clone, Default)]
#[repr(C)]
pub struct HeapStats {
    pool_words: usize,
    pool_max_words: usize,
    pool_live_words: usize,
    pool_live_blocks: usize,
    pool_frag_words: usize,
    large_words: usize,
    large_max_words: usize,
    large_blocks: usize,
}

pub struct CamlHeapState {
    alive: Vec<Vec<Value>>,
    todo: Vec<Vec<Value>>,
    stats: HeapStats,
}

#[repr(C)]
pub struct CustomOperations<'a> {
    identifier: &'a str,
    finalize: Option<extern "C" fn(v: &Value)>,
}

/// always readable by all threads
/// written only by a single thread during STW periods
#[repr(C)]
pub struct GlobalHeapState {
    marked: usize,
    unmarked: usize,
    garbage: usize,
}

#[no_mangle]
pub static mut caml_global_heap_state: GlobalHeapState = GlobalHeapState {
    marked: 0,
    unmarked: 1 << 8,
    garbage: 2 << 8,
};

#[no_mangle]
pub extern "C" fn caml_init_shared_heap() -> Box<CamlHeapState> {
    Box::new(CamlHeapState {
        alive: vec![],
        todo: vec![],
        stats: HeapStats::default(),
    })
}

#[no_mangle]
pub extern "C" fn caml_teardown_shared_heap(_: Box<CamlHeapState>) {
    // no-op thanks to the amazing language that is rust
}

fn alloc(heap: &mut CamlHeapState, wh_size: usize) -> &mut [Value] {
    heap.stats.large_words += wh_size;
    if heap.stats.large_words > heap.stats.large_max_words {
        heap.stats.large_max_words = heap.stats.large_words
    }
    heap.stats.large_blocks += 1;
    heap.alive.push(vec![Value(0); wh_size]);
    heap.alive.last_mut().unwrap()
}

#[no_mangle]
pub extern "C" fn caml_shared_try_alloc(
    heap: &mut CamlHeapState,
    nb_words: size_t,
    tag: size_t,
    _reserved: size_t,
    pinned: bool,
) -> *const Value {
    let wh_size = nb_words + 1;
    let p = alloc(heap, wh_size);
    let colour = if pinned {
        NOT_MARKABLE
    } else {
        unsafe { caml_global_heap_state.marked }
    };
    p[0] = Value((((nb_words) << (8 + 2)) + colour + tag) as u64);
    &p[0]
}

/// Copy the domain-local heap stats into a heap stats sample.
#[no_mangle]
pub extern "C" fn caml_collect_heap_stats_sample(heap: &CamlHeapState, sample: &mut HeapStats) {
    *sample = heap.stats.clone()
}

/// Add the global orphaned heap stats into an accumulator.
#[no_mangle]
pub extern "C" fn caml_accum_orphan_heap_stats() {}

#[no_mangle]
pub extern "C" fn caml_heap_size(heap: &CamlHeapState) -> usize {
    heap.stats.large_words * 8
}

#[no_mangle]
pub extern "C" fn caml_top_heap_words(heap: &CamlHeapState) -> usize {
    heap.stats.large_max_words
}

#[no_mangle]
pub extern "C" fn caml_heap_blocks(heap: &CamlHeapState) -> usize {
    heap.stats.large_blocks
}

#[no_mangle]
pub extern "C" fn caml_pool_of_shared_block(_v: Value) -> &'static Pool {
    unimplemented!()
}

#[no_mangle]
pub extern "C" fn caml_shared_unpin(_v: Value) {
    unimplemented!()
}

#[no_mangle]
pub extern "C" fn caml_redarken_pool(_pool: &Pool) {
    unimplemented!()
}

/// must be called during STW
#[no_mangle]
pub extern "C" fn caml_cycle_heap_stw() {
    let old_g = unsafe { &caml_global_heap_state };
    let new_g = GlobalHeapState {
        marked: old_g.garbage,
        unmarked: old_g.marked,
        garbage: old_g.unmarked,
    };
    unsafe {
        caml_global_heap_state = new_g;
    }
}

/// must be called on each domain
///
/// (after caml_cycle_heap_stw)
#[no_mangle]
pub extern "C" fn caml_cycle_heap(heap: &mut CamlHeapState) {
    swap(&mut heap.alive, &mut heap.todo);
}

fn is_garbage(v: &Value) -> bool {
    let garbo = unsafe { caml_global_heap_state.garbage };
    let malloy = v.0 & (((1 << 2) - 1) << 8);
    garbo as u64 == malloy
}

fn has_custom_tag(hd: Value) -> bool {
    hd.0 & ((1 << 8) - 1) == CUSTOM_TAG
}

fn free(heap: &mut CamlHeapState, a: Vec<Value>) {
    if has_custom_tag(a[0]) {
        let custom = a[1].0 as *const CustomOperations;
        if let Some(f) = unsafe { (*custom).finalize } {
            f(&a[1])
        }
    }
    heap.stats.large_words -= a.len();
    heap.stats.large_blocks -= 1;
    // drop(a) automatically called at this stage
}

fn random_check(p: f64) -> bool {
    // p is probability of success
    let mut rng = rand::thread_rng();
    let random_number = rng.gen::<f64>();
    random_number < p
}

#[no_mangle]
pub extern "C" fn caml_sweep(heap: &mut CamlHeapState, mut work: isize) -> isize {
    if on_strike() && random_check(0.5) {
        eprintln!("The garbage collectors are on strike.");
        return work;
    }

    while work > 0 {
        if let Some(a) = heap.todo.pop() {
            work -= a.len() as isize;
            if is_garbage(&a[0]) {
                free(heap, a)
            } else {
                heap.alive.push(a)
            }
        } else {
            break;
        }
    }
    work
}

/* Heap invariant verification (for debugging) */

/* caml_verify_heap must only be called while all domains are paused */
#[no_mangle]
pub extern "C" fn caml_verify_heap() {
    unimplemented!()
}
