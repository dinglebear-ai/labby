use labby_primitives::action::{ActionSpec, ParamSpec};

const ID: ParamSpec = ParamSpec {
    name: "providerId",
    ty: "string",
    required: true,
    description: "Stable provider ID",
};
const OPERATION: ParamSpec = ParamSpec {
    name: "operationId",
    ty: "string",
    required: true,
    description: "Opaque idempotency operation ID",
};
const VERSION: ParamSpec = ParamSpec {
    name: "expectedVersion",
    ty: "string",
    required: true,
    description: "Expected provider configuration version",
};

pub const ACTIONS: &[ActionSpec] = &[
    ActionSpec {
        name: "providers.list",
        description: "List configured Depot providers",
        destructive: false,
        requires_admin: true,
        params: &[],
        returns: "DepotProvider[]",
    },
    ActionSpec {
        name: "providers.get",
        description: "Get one Depot provider",
        destructive: false,
        requires_admin: true,
        params: &[ID],
        returns: "DepotProvider",
    },
    ActionSpec {
        name: "providers.upsert",
        description: "Create or update a Depot provider",
        destructive: false,
        requires_admin: true,
        params: &[ID, VERSION, OPERATION],
        returns: "DepotProviderMutation",
    },
    ActionSpec {
        name: "providers.remove",
        description: "Remove a provider and its owned active credential",
        destructive: true,
        requires_admin: true,
        params: &[ID, VERSION, OPERATION],
        returns: "DepotProviderMutation",
    },
    ActionSpec {
        name: "providers.probe",
        description: "Diagnose a provider without saving it",
        destructive: false,
        requires_admin: true,
        params: &[ID],
        returns: "DepotProviderProbe",
    },
    ActionSpec {
        name: "operations.get",
        description: "Read a durable provider operation outcome",
        destructive: false,
        requires_admin: true,
        params: &[OPERATION],
        returns: "DepotProviderMutation",
    },
];
