//! `NamedRegistry` — the one generic shape every boot-time extension seam
//! composes with (`#112`). Before this existed, `router::Registry` had its
//! own hand-rolled name -> factory map, and every future seam (the
//! catalog/collection registry backend, a cache tier, …) would have re-decided
//! the same three questions: how names are stored, what an unknown name looks
//! like, and in what order names are listed. This type answers all three
//! once, so a seam's own code is left with only its own domain logic —
//! `router::Registry` is the first adopter (see its own doc).
//!
//! Three properties, matching the extension-model decision record
//! (`docs/design/2026-07-21-extension-model-boot-time-registries.md`):
//!
//! - **Named, not discovered.** An entry exists only because something called
//!   [`register`](NamedRegistry::register) with an explicit name — never
//!   because a crate happened to link a constructor that added itself.
//! - **Refuse by name.** [`get`](NamedRegistry::get) returns `None` for a name
//!   that was never registered — indistinguishable, on purpose, from a name
//!   whose crate was compiled out entirely. Both are "this binary does not
//!   contain that," and the caller (which already knows the config key that
//!   produced the name) is in a better position than this generic type to
//!   phrase the resulting error precisely.
//! - **Deterministic iteration.** [`names`](NamedRegistry::names) always
//!   yields entries in the same order regardless of registration order — the
//!   order a boot log enumerating "what this binary actually contains" can
//!   rely on run to run.
//!
//! Registering the same name twice replaces the earlier entry — the same
//! last-write-wins behavior `router::Registry`'s own map already had before
//! this type existed, kept unchanged rather than turned into a new error
//! case no caller asked for.

use std::collections::BTreeMap;
use std::sync::Arc;

/// A name -> `Arc<T>` map with deterministic iteration, generic over the
/// capability trait object a seam registers (`Arc<dyn DriverFactory>`,
/// `Arc<dyn RelationalRegistryFactory>`, …). Deliberately minimal: no
/// lifecycle hooks, no async construction, no priority — a seam that needs
/// more than "look up by name, list what's here" is not this type's job (see
/// the decision record's "kept small on purpose" section).
pub struct NamedRegistry<T: ?Sized> {
    entries: BTreeMap<String, Arc<T>>,
}

impl<T: ?Sized> Default for NamedRegistry<T> {
    fn default() -> Self {
        Self {
            entries: BTreeMap::new(),
        }
    }
}

impl<T: ?Sized> NamedRegistry<T> {
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers `item` under `name`, replacing whatever was previously
    /// registered under the same name.
    pub fn register(&mut self, name: impl Into<String>, item: Arc<T>) {
        self.entries.insert(name.into(), item);
    }

    /// The entry registered under `name`, or `None` when nothing is — the
    /// caller turns that into its own precisely-worded error (see this
    /// module's own doc for why the wording is the caller's job, not this
    /// type's).
    pub fn get(&self, name: &str) -> Option<&Arc<T>> {
        self.entries.get(name)
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Every registered name, alphabetically — the same order on every call
    /// regardless of registration order, which is what lets a boot log
    /// enumerate "what this binary actually contains" without that line's
    /// content depending on the order `register` happened to be called in.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.entries.keys().map(String::as_str)
    }

    /// `(name, item)` pairs in the same deterministic order as
    /// [`names`](Self::names).
    pub fn iter(&self) -> impl Iterator<Item = (&str, &Arc<T>)> {
        self.entries
            .iter()
            .map(|(name, item)| (name.as_str(), item))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    trait Greeter: Send + Sync {
        fn greet(&self) -> String;
    }

    struct Fixed(&'static str);

    impl Greeter for Fixed {
        fn greet(&self) -> String {
            self.0.to_string()
        }
    }

    #[test]
    fn get_returns_none_for_a_name_never_registered() {
        let registry: NamedRegistry<dyn Greeter> = NamedRegistry::new();
        assert!(registry.get("nope").is_none());
    }

    #[test]
    fn get_returns_the_registered_entry() {
        let mut registry: NamedRegistry<dyn Greeter> = NamedRegistry::new();
        registry.register("hello", Arc::new(Fixed("hi")));
        assert_eq!(registry.get("hello").unwrap().greet(), "hi");
    }

    #[test]
    fn registering_the_same_name_twice_replaces_the_earlier_entry() {
        let mut registry: NamedRegistry<dyn Greeter> = NamedRegistry::new();
        registry.register("hello", Arc::new(Fixed("first")));
        registry.register("hello", Arc::new(Fixed("second")));
        assert_eq!(registry.get("hello").unwrap().greet(), "second");
        assert_eq!(registry.len(), 1);
    }

    #[test]
    fn names_are_alphabetical_regardless_of_registration_order() {
        let mut registry: NamedRegistry<dyn Greeter> = NamedRegistry::new();
        registry.register("zebra", Arc::new(Fixed("z")));
        registry.register("alpha", Arc::new(Fixed("a")));
        registry.register("mid", Arc::new(Fixed("m")));
        let names: Vec<&str> = registry.names().collect();
        assert_eq!(names, vec!["alpha", "mid", "zebra"]);
    }

    #[test]
    fn empty_registry_reports_empty_and_zero_len() {
        let registry: NamedRegistry<dyn Greeter> = NamedRegistry::new();
        assert!(registry.is_empty());
        assert_eq!(registry.len(), 0);
        assert_eq!(registry.names().count(), 0);
    }

    #[test]
    fn iter_yields_names_paired_with_their_entries_in_the_same_order_as_names() {
        let mut registry: NamedRegistry<dyn Greeter> = NamedRegistry::new();
        registry.register("b", Arc::new(Fixed("bravo")));
        registry.register("a", Arc::new(Fixed("alpha")));
        let pairs: Vec<(&str, String)> = registry.iter().map(|(n, g)| (n, g.greet())).collect();
        assert_eq!(
            pairs,
            vec![("a", "alpha".to_string()), ("b", "bravo".to_string())]
        );
    }
}
