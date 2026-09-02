use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use tellurion_core::{
    AttributeColumn, CatalogSource, CollectionDecl, DriverFactory, FeaturePage, FeatureSource,
    ItemsQuery, PhysicalCollection, Result, SpatialExtent, StorageDecl, StorageDriver,
};

use crate::{MemoryDataset, MemoryDriverError};

/// An immutable storage driver over prevalidated [`MemoryDataset`] values.
#[derive(Clone)]
pub struct MemoryDriver {
    backend: Arc<MemoryBackend>,
}

impl MemoryDriver {
    /// Builds a driver and rejects duplicate physical collection names.
    pub fn new(
        datasets: impl IntoIterator<Item = MemoryDataset>,
    ) -> std::result::Result<Self, MemoryDriverError> {
        let mut by_name = BTreeMap::new();
        for dataset in datasets {
            let name = dataset.name().to_string();
            if by_name.insert(name.clone(), dataset).is_some() {
                return Err(MemoryDriverError::Configuration(format!(
                    "duplicate memory dataset '{name}'"
                )));
            }
        }
        Ok(Self {
            backend: Arc::new(MemoryBackend { datasets: by_name }),
        })
    }
}

impl StorageDriver for MemoryDriver {
    fn catalog_source(&self) -> Arc<dyn CatalogSource> {
        Arc::clone(&self.backend) as Arc<dyn CatalogSource>
    }

    fn feature_source(&self) -> Option<Arc<dyn FeatureSource>> {
        Some(Arc::clone(&self.backend) as Arc<dyn FeatureSource>)
    }

    // Tiles, volumes, writes, and outbox reads use their honest default None.
}

/// A fixture factory whose drivers are preloaded by `StorageDecl.id`.
#[derive(Default)]
pub struct MemoryDriverFactory {
    drivers: BTreeMap<String, Arc<MemoryDriver>>,
}

impl MemoryDriverFactory {
    /// Creates an empty factory.
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers one driver for a storage id, rejecting duplicate ids.
    pub fn insert(
        &mut self,
        storage_id: impl Into<String>,
        driver: MemoryDriver,
    ) -> std::result::Result<(), MemoryDriverError> {
        let storage_id = storage_id.into();
        if self.drivers.contains_key(&storage_id) {
            return Err(MemoryDriverError::Configuration(format!(
                "duplicate memory storage registration '{storage_id}'"
            )));
        }
        self.drivers.insert(storage_id, Arc::new(driver));
        Ok(())
    }
}

impl DriverFactory for MemoryDriverFactory {
    fn name(&self) -> &str {
        "memory"
    }

    fn build(&self, decl: &StorageDecl) -> Result<Arc<dyn StorageDriver>> {
        self.drivers
            .get(&decl.id)
            .map(|driver| Arc::clone(driver) as Arc<dyn StorageDriver>)
            .ok_or_else(|| {
                MemoryDriverError::Configuration(format!(
                    "storage '{}': no preloaded memory driver is registered",
                    decl.id
                ))
                .into()
            })
    }
}

struct MemoryBackend {
    datasets: BTreeMap<String, MemoryDataset>,
}

impl MemoryBackend {
    fn dataset(&self, name: &str) -> Result<&MemoryDataset> {
        self.datasets.get(name).ok_or_else(|| {
            MemoryDriverError::Configuration(format!("memory dataset '{name}' is not registered"))
                .into()
        })
    }

    fn collection_name(collection: &CollectionDecl) -> &str {
        collection.table.as_deref().unwrap_or(&collection.id)
    }
}

#[async_trait]
impl CatalogSource for MemoryBackend {
    async fn collections(&self) -> Result<Vec<PhysicalCollection>> {
        Ok(self
            .datasets
            .values()
            .map(|dataset| PhysicalCollection {
                name: dataset.name().to_string(),
                geometry_column: Some("geometry".into()),
                primary_key: Some("id".into()),
                srid: Some(4326),
                geometry_type: dataset.geometry_type().map(str::to_string),
            })
            .collect())
    }

    async fn extent(&self, physical: &PhysicalCollection) -> Result<Option<SpatialExtent>> {
        Ok(self
            .dataset(&physical.name)?
            .extent()
            .map(|bbox| SpatialExtent { bbox }))
    }

    async fn row_estimate(&self, physical: &PhysicalCollection) -> Result<Option<u64>> {
        Ok(Some(self.dataset(&physical.name)?.len() as u64))
    }

    async fn attribute_schema(
        &self,
        physical: &PhysicalCollection,
    ) -> Result<Option<Vec<AttributeColumn>>> {
        Ok(Some(
            self.dataset(&physical.name)?
                .attribute_schema()
                .into_iter()
                .map(|(name, sql_type)| AttributeColumn {
                    name: name.to_string(),
                    sql_type: sql_type.to_string(),
                })
                .collect(),
        ))
    }
}

#[async_trait]
impl FeatureSource for MemoryBackend {
    async fn items(&self, collection: &CollectionDecl, query: &ItemsQuery) -> Result<FeaturePage> {
        self.dataset(Self::collection_name(collection))?
            .items(query)
    }

    async fn item(
        &self,
        collection: &CollectionDecl,
        id: &str,
        _filter: Option<&tellurion_core::Filter>,
    ) -> Result<Option<serde_json::Value>> {
        // This driver never overrides `filter_capable` (trait default,
        // `false`), so the serving handlers refuse a filtered-only grant
        // before this is ever called with `Some` — same convention as the
        // other non-filter-capable drivers in this workspace.
        Ok(self.dataset(Self::collection_name(collection))?.item(id))
    }
}
