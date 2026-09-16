# Browser administration (preview)

The production control workspace requires configured browser OIDC, a durable
control store, and an authorized stored role binding for the signed-in identity.
Open the URL for the scope you administer:

| Workspace | URL | Built-in role at that scope |
|---|---|---|
| Platform | `/ui/control` | `sysadmin` at platform scope |
| Tenant | `/ui/control/tenants/{tenant}` | `tenant_admin` at that tenant |
| Catalog | `/ui/control/tenants/{tenant}/catalogs/{catalog}` | `catalog_admin` at that catalog |

`{tenant}` and `{catalog}` are external IDs of existing entities. A platform
`sysadmin` can also enter scoped workspaces. Custom policies may grant access,
but a role name or token claim alone does not: the server checks the signed-in
identity against current durable authorization for the requested workspace.
These are direct links, so a scoped administrator need not enter the platform
workspace first.

The platform workspace shows tenant inventory and effective settings; scoped
workspaces show catalog or collection inventory. Inventory is read-only in the
browser. The only production
editor here changes the current scope's `cache_ttl_s` setting (cache lifetime
in seconds). Leaving the field blank unsets that scope's value; it does not
delete the tenant or catalog, alter a sibling scope, or expose other
configuration fields for editing.

To change the lifetime, edit the field, select **Preview changes**, inspect the
proposed revision and changed resource, then select **Apply previewed settings**.
The editor uses the settings entity version to detect intervening changes. If
there is a conflict, rebase the retained draft onto freshly loaded settings
and preview again before applying. If an apply result is uncertain, retry the
exact request before making another edit. A successful durable commit reports
a control revision; it does **not** by itself prove that every running instance
has activated it. Check the applied-revision and revision-lag metrics described
in the [README](../README.md#quickstart) when confirming rollout.

The fixture at `/ui/control-demo` is separate from production administration.
It simulates a settings preview locally and never writes a control record.
The public-demo-only server does not expose production control endpoints.
