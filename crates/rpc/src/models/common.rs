use std::marker::PhantomData;

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Id<T> {
    pub value: i32,
    pub _marker: PhantomData<fn() -> T>,
}

impl<T> Id<T> {
    pub fn new(v: i32) -> Self {
        Self {
            value: v,
            _marker: PhantomData,
        }
    }
}
