//! The `strict`-mode marker trait.
//!
//! `TsType` has no methods and no runtime cost. Its only purpose is turning an
//! unmapped field type into a `rustc` error instead of a silent `unknown` in the
//! generated TypeScript.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};

pub trait TsType {}

macro_rules! impl_ts_type {
    ($($t:ty),* $(,)?) => { $(impl TsType for $t {})* };
}

impl_ts_type!(
    bool,
    char,
    str,
    String,
    i8,
    i16,
    i32,
    i64,
    i128,
    isize,
    u8,
    u16,
    u32,
    u64,
    u128,
    usize,
    f32,
    f64,
    std::path::PathBuf,
    (),
);

impl<T: TsType + ?Sized> TsType for &T {}
impl<T: TsType + ?Sized> TsType for Box<T> {}
impl<T: TsType + ?Sized> TsType for std::rc::Rc<T> {}
impl<T: TsType + ?Sized> TsType for std::sync::Arc<T> {}
impl<T: TsType> TsType for Option<T> {}
impl<T: TsType> TsType for Vec<T> {}
impl<T: TsType> TsType for VecDeque<T> {}
impl<T: TsType> TsType for [T] {}
impl<T: TsType, const N: usize> TsType for [T; N] {}
impl<T: TsType> TsType for HashSet<T> {}
impl<T: TsType> TsType for BTreeSet<T> {}
impl<K: TsType, V: TsType> TsType for HashMap<K, V> {}
impl<K: TsType, V: TsType> TsType for BTreeMap<K, V> {}
impl<T: TsType, E: TsType> TsType for Result<T, E> {}
impl<A: TsType, B: TsType> TsType for (A, B) {}
impl<A: TsType, B: TsType, C: TsType> TsType for (A, B, C) {}
