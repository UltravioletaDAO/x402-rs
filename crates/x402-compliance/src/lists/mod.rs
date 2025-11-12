pub mod ofac;
pub mod blacklist;

use crate::checker::ListMetadata;

/// Trait that all sanctions lists must implement
pub trait SanctionsList: Send + Sync {
    /// Check if an address is sanctioned
    fn is_sanctioned(&self, address: &str) -> bool;

    /// Get metadata about this list
    fn metadata(&self) -> ListMetadata;

    /// Get the total number of addresses in the list
    fn total_addresses(&self) -> usize;
}
