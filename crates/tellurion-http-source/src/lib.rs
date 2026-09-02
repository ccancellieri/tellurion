//! Bounded HTTPS byte-range objects for untrusted public locators.

mod address;
#[cfg(feature = "administrative-compat")]
mod administrative;
mod budget;
mod error;
mod range;
mod url;

#[cfg(test)]
mod range_tests;

pub use address::is_public_address;
#[cfg(feature = "administrative-compat")]
pub use administrative::{AdministrativeRangeObject, AdministrativeSourceError};
pub use budget::{Budget, BudgetError, BudgetErrorKind, BudgetLimits, BudgetReservation};
pub use error::{SourceError, SourceErrorKind};
pub use range::{ContentIdentity, PublicHttpsGateway, RangeObject, SourceHandle, SourceSession};
pub use url::{validate_public_url, PublicUrl, UrlValidationError};
