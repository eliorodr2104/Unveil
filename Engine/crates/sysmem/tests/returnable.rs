// Modified for Unveil (2026-10-17): memory ownership and reserve policy; see Engine/CHANGES.md.
use lightcraft_sysmem::ReturnableVec;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

#[test]
fn checked_capacity_and_normal_vector_lifetimes() {
    let mut values = ReturnableVec::<u32>::new_in(lightcraft_sysmem::Returnable);
    assert!(values.try_reserve_exact(usize::MAX).is_err());
    assert!(values.is_empty());
    values.try_reserve_exact(300_000).unwrap();
    values.extend((0..300_000).map(|n| n ^ 0x1357));
    let shared = Arc::new(values);
    let borrowed = shared.clone();
    drop(shared);
    assert_eq!(borrowed[299_999], 299_999 ^ 0x1357);
    let copy = (*borrowed).clone();
    drop(borrowed);
    assert_eq!(copy[123_456], 123_456 ^ 0x1357);
}

#[test]
fn elements_drop_once_on_truncate_and_final_release() {
    #[derive(Clone)]
    struct Item(Arc<AtomicUsize>);
    impl Drop for Item {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
    }
    let drops = Arc::new(AtomicUsize::new(0));
    let mut values = ReturnableVec::new_in(lightcraft_sysmem::Returnable);
    values.try_reserve_exact(200_000).unwrap();
    for _ in 0..200_000 {
        values.push(Item(drops.clone()));
    }
    values.truncate(100_000);
    assert_eq!(drops.load(Ordering::Relaxed), 100_000);
    drop(values);
    assert_eq!(drops.load(Ordering::Relaxed), 200_000);
}

#[test]
fn zero_size_and_overaligned_elements_remain_valid() {
    #[derive(Clone)]
    #[repr(align(65536))]
    struct Aligned(u8);
    let mut aligned = ReturnableVec::new_in(lightcraft_sysmem::Returnable);
    aligned.resize(32, Aligned(7));
    assert_eq!(aligned.as_ptr() as usize % 65536, 0);
    assert_eq!(aligned[31].0, 7);
    let mut empty = ReturnableVec::new_in(lightcraft_sysmem::Returnable);
    empty.resize(1_000_000, ());
    assert_eq!(empty.len(), 1_000_000);
}

#[test]
fn growing_and_shrinking_across_the_mapping_threshold_preserves_elements() {
    let mut values = ReturnableVec::new_in(lightcraft_sysmem::Returnable);
    values.extend(0u32..100);
    values.reserve_exact(300_000);
    assert_eq!(&values[..], &(0u32..100).collect::<Vec<_>>());
    values.extend(100u32..300_000);
    values.truncate(100);
    values.shrink_to_fit();
    assert_eq!(values.capacity(), 100);
    assert_eq!(&values[..], &(0u32..100).collect::<Vec<_>>());
    values.reserve_exact(400_000);
    assert_eq!(values[99], 99);
}
