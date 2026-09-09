# Wire-only compatibility check; no Depot application, database or HTTP server.
# Requires Elixir 1.18+ with its standard JSON module and OTP Ed25519 support.
# Run: elixir crates/labby-auth/tests/fixtures/verify_managed_depot.exs /path/to/depot
# The verifier is read from the fixture's exact Git revision, not the checkout.
# Jason and Auth are narrow test adapters for JSON decoding and scope constants;
# the pinned Depot.Delegation verifier itself is compiled without modification.
defmodule Jason do
  def decode(bytes), do: JSON.decode(bytes)
end

defmodule Depot.Auth do
  def read_scope, do: "skills:read"
  def write_scope, do: "skills:write"
end

defmodule Depot.Operations.Registry do
  def definition(_), do: raise("operation dispatch is outside this wire-only check")
  def idempotent?(_), do: raise("replay handling is outside this wire-only check")
end

[depot_repo] = System.argv()
fixture = __DIR__ |> Path.join("managed-depot-v1.json") |> File.read!() |> JSON.decode!()
{source, 0} = System.cmd("git", ["-C", depot_repo, "show", fixture["depot_revision"] <> ":lib/depot/delegation.ex"])
Code.compile_string(source, "pinned-depot-delegation.ex")
claims = fixture["claims"]
Application.put_env(:depot, :control_plane_identity, %{deployment_id: "d1", account_id: "a1"})
Application.put_env(:depot, :delegation_public_keys, %{"current" => Base.url_decode64!(fixture["public_key_base64url"], padding: false)})
expected = %{
  method: "GET", resource: "/api/artifacts", operation: "depot.artifacts.list",
  intent_id: "i1", organization_id: "o1", team_id: "t1", project_id: "pr1",
  required_scope: "skills:read"
}
token = fixture["token"]
now = fixture["now"]
{:ok, ^claims} = Depot.Delegation.verify(token, expected, now)
for {key, value, reason} <- [
  {:method, "POST", :method_mismatch},
  {:resource, "/api/other", :resource_mismatch},
  {:operation, "depot.artifacts.delete", :operation_mismatch},
  {:intent_id, "other", :intent_mismatch},
  {:organization_id, "other", :organization_mismatch},
  {:team_id, "other", :team_mismatch},
  {:project_id, "other", :project_mismatch},
  {:required_scope, "skills:write", :insufficient_scope}
] do
  {:error, ^reason} = Depot.Delegation.verify(token, Map.put(expected, key, value), now)
end
{:error, :token_expired} = Depot.Delegation.verify(token, expected, 100)
[header, body, _signature] = String.split(token, ".")
bad_signature = Base.url_encode64(<<0::512>>, padding: false)
{:error, :invalid_signature} = Depot.Delegation.verify(header <> "." <> body <> "." <> bad_signature, expected, now)
Application.put_env(:depot, :control_plane_identity, %{deployment_id: "other", account_id: "a1"})
{:error, :identity_mismatch} = Depot.Delegation.verify(token, expected, now)
IO.puts("Managed Depot wire fixture: signature accepted; 11 negative bindings rejected. No authorization or live transport exercised.")
