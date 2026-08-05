export const capabilities = {
  schema_version: 1,
  product: "axio",
  display_name: "Axio",
  parity: {
    state: "complete",
    legacy_admin: null,
    redirect_ready: true,
  },
  modules: [
    {
      id: "overview",
      label: "Overview",
      permission: "overview.read",
      actions: [],
      columns: [
        { name: "lifecycle", label: "Lifecycle", type: "text", primary: true },
        { name: "domain", label: "Domain", type: "text", primary: true },
        { name: "surface", label: "Surface", type: "text", primary: true },
        { name: "runtime", label: "Runtime", type: "text", primary: true },
        { name: "identity_authority", label: "Identity", type: "text" },
        { name: "adapter_mode", label: "Adapter", type: "text" },
      ],
    },
  ],
};

export const overview = {
  data: [
    {
      id: "axio",
      lifecycle: "active",
      domain: "axio.sh",
      surface: "Product site and install scripts",
      runtime: "Local-first CLI",
      identity_authority: "Umbra",
      product_accounts: "Not implemented",
      adapter_mode: "Read-only",
    },
  ],
};
