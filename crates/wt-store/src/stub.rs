//! Placeholder implementation of [`Store`], replaced by the real
//! on-disk store in ticket 04. Exists so downstream crates compile
//! against the trait before the store lands.

use std::io;

use crate::{ContentId, Error, Result, Store};

#[derive(Debug, Default)]
pub struct StubStore;

impl Store for StubStore {
    fn put(&mut self, _content: &[u8]) -> Result<ContentId> {
        Err(Error::Io(io::Error::other("store not implemented yet")))
    }

    fn get(&self, id: &ContentId) -> Result<Vec<u8>> {
        Err(Error::UnknownContent(*id))
    }

    fn contains(&self, _id: &ContentId) -> bool {
        false
    }

    fn add_ref(&mut self, id: &ContentId) -> Result<()> {
        Err(Error::UnknownContent(*id))
    }

    fn release_ref(&mut self, id: &ContentId) -> Result<()> {
        Err(Error::RefCountUnderflow(*id))
    }

    fn ref_count(&self, id: &ContentId) -> Result<u64> {
        Err(Error::UnknownContent(*id))
    }
}
