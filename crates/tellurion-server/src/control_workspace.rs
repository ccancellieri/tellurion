use tellurion_control::ControlRouteDescriptor;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ControlWorkspace<'a> {
    Platform,
    Tenant(&'a str),
    Catalog(&'a str, &'a str),
}

impl<'a> ControlWorkspace<'a> {
    pub(crate) fn parse(path: &'a str) -> Option<Self> {
        let segments = path.split('/').collect::<Vec<_>>();
        match segments.as_slice() {
            ["", "ui", "control"] => Some(Self::Platform),
            ["", "ui", "control", "tenants", tenant] if valid_slug(tenant) => {
                Some(Self::Tenant(tenant))
            }
            ["", "ui", "control", "tenants", tenant, "catalogs", catalog]
                if valid_slug(tenant) && valid_slug(catalog) =>
            {
                Some(Self::Catalog(tenant, catalog))
            }
            _ => None,
        }
    }

    pub(crate) fn admission(&self) -> (ControlRouteDescriptor, String) {
        let descriptor = match self {
            Self::Platform => ControlRouteDescriptor::PlatformOverview,
            Self::Tenant(_) => ControlRouteDescriptor::TenantCatalogs,
            Self::Catalog(_, _) => ControlRouteDescriptor::CatalogCollections,
        };
        let path = match self {
            Self::Platform => descriptor.template().to_string(),
            Self::Tenant(tenant) => format!("/_control/v1/tenants/{tenant}/catalogs"),
            Self::Catalog(tenant, catalog) => {
                format!("/_control/v1/tenants/{tenant}/catalogs/{catalog}/collections")
            }
        };
        (descriptor, path)
    }
}

fn valid_slug(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_exact_control_workspaces_have_admission_descriptors() {
        for (path, template, admission_path) in [
            (
                "/ui/control",
                "/_control/v1/platform/overview",
                "/_control/v1/platform/overview",
            ),
            (
                "/ui/control/tenants/acme",
                "/_control/v1/tenants/{tenant}/catalogs",
                "/_control/v1/tenants/acme/catalogs",
            ),
            (
                "/ui/control/tenants/acme/catalogs/roads",
                "/_control/v1/tenants/{tenant}/catalogs/{catalog}/collections",
                "/_control/v1/tenants/acme/catalogs/roads/collections",
            ),
        ] {
            let (descriptor, actual) = ControlWorkspace::parse(path).unwrap().admission();
            assert_eq!(descriptor.template(), template);
            assert_eq!(actual, admission_path);
        }
        for path in [
            "/ui/control/",
            "/ui/control/tenants",
            "/ui/control/tenants/acme/settings",
            "/ui/control/tenants/acme/catalogs",
            "/ui/control/tenants/acme/catalogs/roads/collections",
            "/ui/control/tenants/../catalogs/roads",
            "/ui/control/tenants/a%2Fb",
            "/ui/control/tenants/a.b",
            "/ui/control/tenants/a//catalogs/b",
            "/ui/control/tenants/a/catalogs/b?x=1",
            "/ui/control-demo",
        ] {
            assert!(ControlWorkspace::parse(path).is_none(), "{path}");
        }
    }
}
