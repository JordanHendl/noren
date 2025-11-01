use std::collections::HashSet;

#[derive(Default)]
pub struct DataCache<T> {
    data: HashSet<T>,
}

impl<T> DataCache<T> {
    pub fn new() -> Self {
        Self {
            data: Default::default()
        }
    }
}
