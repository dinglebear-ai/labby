defmodule LabbyConsole.AccessTest do
  use ExUnit.Case, async: true
  alias LabbyConsole.Access
  alias LabbyConsole.GatewayAction.Error

  test "only authenticated finite help entries enable actions" do
    assert {:ok, names} =
             Access.decode_help(%{
               "service" => "access",
               "actions" => [
                 %{"name" => "access.team.create"},
                 %{"name" => "access.future.delete"}
               ]
             })

    assert MapSet.member?(names, "access.team.create")
    refute MapSet.member?(names, "access.future.delete")
    assert {:error, %Error{}} = Access.decode_help(%{"service" => "gateway", "actions" => []})

    assert {:error, %Error{}} =
             Access.decode_help(%{
               "service" => "access",
               "actions" => List.duplicate(%{"name" => "x"}, 65)
             })
  end

  test "invitation validation returns only a nonsecret intent and never retains token" do
    secret = String.duplicate("ab", 32)

    assert {:ok, params, intent} =
             Access.prepare("access.team_invitation.create", %{
               "team_id" => "t",
               "principal_id" => "p",
               "role" => "member",
               "token" => secret,
               "ttl_seconds" => "60"
             })

    assert params["ttl_seconds"] == 60
    refute inspect(intent) =~ secret
    refute Map.has_key?(intent.params, "token")

    assert {:error, _} =
             Access.prepare("access.team_invitation.accept", %{
               "token" => String.duplicate("0", 64)
             })

    assert {:error, _} =
             Access.prepare(
               "access.team_invitation.create",
               Map.put(params, "ttl_seconds", 604_801)
             )

    assert {:error, _} =
             Access.prepare("access.team.create", %{"team_id" => " t", "name" => "Team"})

    assert {:error, _} =
             Access.prepare("access.team.create", %{
               "team_id" => "t",
               "name" => "Team",
               "actor" => "p"
             })
  end

  test "unknown invitation outcome cannot be reconciled by duplicate visible metadata" do
    {:ok, _, intent} =
      Access.prepare("access.team_invitation.create", %{
        "team_id" => "t",
        "principal_id" => "p",
        "role" => "member",
        "token" => String.duplicate("ab", 32),
        "ttl_seconds" => 60
      })

    state =
      Access.new()
      |> Access.write_result(
        intent,
        {:error, Error.new(:timeout, "access", intent.action, side_effects: "unknown")}
      )

    assert state.receipt.status == :uncertain

    state =
      Access.read_result(
        state,
        :invitations,
        {:ok,
         %{
           rows: [%{"principal_id" => "p", "role" => "member", "status" => "pending"}],
           next_offset: nil,
           revision: 1,
           metadata: %{}
         }},
        %{"team_id" => "t", "offset" => 0, "limit" => 50}
      )

    assert state.receipt.status == :uncertain
    refute Access.writable?(state)
  end

  test "unrelated read never clears a held write and forbidden removes only affected data" do
    {:ok, _, intent} =
      Access.prepare("access.team.member.suspend", %{"team_id" => "t", "principal_id" => "p"})

    state = Access.new() |> Access.write_result(intent, {:ok, %{"ok" => true}})
    result = {:ok, %{rows: [], next_offset: nil, revision: nil, metadata: %{}}}
    state = Access.read_result(state, :admins, result, %{})
    assert state.receipt.status == :accepted

    state =
      Access.read_result(
        state,
        :members,
        {:error, Error.new(:forbidden, "access", "access.team.member.list", status: 403)},
        %{"team_id" => "t"}
      )

    assert state.cards.members.status == :forbidden
    assert state.receipt.status == :visibility_lost
    assert state.receipt.message =~ "cannot be inspected"
  end

  test "inventory paging retains one revision and forbids mixed scopes and malformed envelopes" do
    body = %{
      "team_id" => "t",
      "team_status" => "active",
      "team_policy_epoch" => 2,
      "team_membership_epoch" => 4,
      "global_revision" => 6,
      "members" => [
        %{
          "principal_id" => "p",
          "role" => "owner",
          "status" => "suspended",
          "membership_epoch" => 3
        }
      ],
      "next_offset" => 1
    }

    assert {:ok, page} =
             Access.decode(:members, body, %{"team_id" => "t", "offset" => 0, "limit" => 1})

    assert page.revision == 6
    assert page.metadata["team_membership_epoch"] == 4

    assert {:error, _} =
             Access.decode(:members, body, %{"team_id" => "other", "offset" => 0, "limit" => 1})

    assert {:error, _} =
             Access.decode(:members, Map.put(body, "next_offset", 0), %{
               "team_id" => "t",
               "offset" => 0,
               "limit" => 1
             })
  end

  test "malformed write success never becomes accepted and token-bearing output is discarded" do
    {:ok, _, intent} =
      Access.prepare("access.team.member.suspend", %{"team_id" => "t", "principal_id" => "p"})

    assert {:error, %Error{kind: :invalid_response, side_effects: "unknown"}} =
             Access.decode_write(intent, {:ok, %{}})

    assert {:ok, %{}} = Access.decode_write(intent, {:ok, %{"ok" => true, "token" => "private"}})
  end

  test "member state read and independent Team visibility read reconcile the same accepted intent" do
    {:ok, _, intent} =
      Access.prepare("access.team.member.suspend", %{"team_id" => "t", "principal_id" => "p"})

    state = Access.new() |> Access.write_result(intent, {:ok, %{}})

    state =
      Access.read_result(
        state,
        :members,
        {:ok,
         %{
           rows: [%{"principal_id" => "p", "status" => "suspended", "role" => "member"}],
           next_offset: nil,
           revision: 2,
           metadata: %{}
         }},
        %{"team_id" => "t"}
      )

    state =
      Access.read_result(
        state,
        :teams,
        {:ok,
         %{
           rows: [%{"team_id" => "t", "status" => "active", "role" => "owner"}],
           next_offset: nil,
           revision: nil,
           metadata: %{}
         }},
        %{}
      )

    assert state.receipt.status == :reconciled
  end

  test "credential inventory retains authorized reference and custodian metadata while dropping unexpected secrets" do
    data = %{
      "bindings" => [
        %{
          "team_id" => "t",
          "upstream_name" => "up",
          "binding_id" => "reference-a",
          "custodian_principal_id" => "custodian",
          "generation" => 2,
          "rotated_at_millis" => 5,
          "status" => "active",
          "token" => "unexpected-secret"
        }
      ]
    }

    assert {:ok, page} = Access.decode(:credentials, data, %{"team_id" => "t"})
    assert hd(page.rows)["binding_id"] == "reference-a"
    assert hd(page.rows)["custodian_principal_id"] == "custodian"
    refute inspect(page) =~ "unexpected-secret"
  end

  test "unrelated forbidden Team snapshot cannot release a held write" do
    {:ok, _, intent} =
      Access.prepare("access.team.member.suspend", %{"team_id" => "t", "principal_id" => "p"})

    state = Access.new() |> Access.write_result(intent, {:ok, %{}})

    state =
      Access.read_result(
        state,
        :members,
        {:error, Error.new(:forbidden, "access", "access.team.member.list", status: 403)},
        %{"team_id" => "different"}
      )

    assert state.receipt.status == :accepted
  end

  test "invitation uncertainty survives lost inventory visibility" do
    {:ok, _, intent} =
      Access.prepare("access.team_invitation.create", %{
        "team_id" => "t",
        "principal_id" => "p",
        "role" => "member",
        "ttl_seconds" => 60,
        "token" => String.duplicate("ab", 32)
      })

    state =
      Access.new()
      |> Access.write_result(
        intent,
        {:error, Error.new(:timeout, "access", intent.action, side_effects: "unknown")}
      )

    state =
      Access.read_result(
        state,
        :invitations,
        {:error, Error.new(:forbidden, "access", "access.team_invitation.list", status: 403)},
        %{"team_id" => "t"}
      )

    assert state.receipt.status == :uncertain
  end

  test "a mismatched credential reference response remains an uncertain write" do
    {:ok, _, intent} =
      Access.prepare("access.gateway_credential.bind", %{
        "team_id" => "t",
        "upstream_name" => "up",
        "binding_id" => "intended-reference"
      })

    data = %{
      "team_id" => "t",
      "upstream_name" => "up",
      "binding_id" => "different-reference",
      "generation" => 2,
      "rotated_at_millis" => 5,
      "status" => "active"
    }

    assert {:error, %Error{side_effects: "unknown"}} = Access.decode_write(intent, {:ok, data})
  end

  test "malformed authority and denied current administrator inventory cannot grant platform controls" do
    refute Access.platform?(%{"is_admin" => true, "capabilities" => "platform.manage"})
    state = Access.new() |> Map.put(:capabilities, MapSet.new(["access.team.create"]))
    state = put_in(state.cards.admins.status, :forbidden)

    assert Access.availability(state, "access.team.create", %{
             "is_admin" => true,
             "capabilities" => ["platform.manage"]
           }) == :forbidden
  end

  test "active nonplatform Team administrators can suspend while suspended Team activation stays restricted" do
    state =
      Access.new()
      |> Map.put(:capabilities, MapSet.new(~w(access.team.suspend access.team.activate)))
      |> Map.put(:team, "t")

    state =
      put_in(state.cards.teams, %{
        state.cards.teams
        | status: :loaded,
          rows: [%{"team_id" => "t", "role" => "admin", "status" => "active"}]
      })

    assert Access.availability(state, "access.team.suspend", %{}) == :available

    state =
      put_in(state.cards.teams.rows, [
        %{"team_id" => "t", "role" => "admin", "status" => "suspended"}
      ])

    assert Access.availability(state, "access.team.activate", %{}) == :forbidden

    state =
      put_in(state.cards.teams.rows, [
        %{"team_id" => "t", "role" => "member", "status" => "active"}
      ])

    assert Access.availability(state, "access.team.suspend", %{}) == :forbidden
  end

  test "binding read-back proves reference and rotation state without claiming which attempt caused it" do
    baseline = %{
      "team_id" => "t",
      "upstream_name" => "up",
      "binding_id" => "ref-a",
      "custodian_principal_id" => "custodian",
      "generation" => 4,
      "rotated_at_millis" => 5,
      "status" => "active"
    }

    state = Access.new() |> Map.put(:team, "t")

    state =
      put_in(state.cards.credentials, %{
        state.cards.credentials
        | status: :loaded,
          rows: [baseline]
      })

    for target <- ["ref-a", "ref-b"] do
      {:ok, _, intent} =
        Access.prepare("access.gateway_credential.bind", %{
          "team_id" => "t",
          "upstream_name" => "up",
          "binding_id" => target
        })

      intent = Access.capture_intent(state, intent)

      unknown =
        Access.write_result(
          state,
          intent,
          {:error, Error.new(:timeout, "access", intent.action, side_effects: "unknown")}
        )

      {:ok, page} = Access.decode(:credentials, %{"bindings" => [baseline]}, %{"team_id" => "t"})

      assert Access.read_result(unknown, :credentials, {:ok, page}, %{"team_id" => "t"}).receipt.status ==
               :uncertain

      desired = Map.merge(baseline, %{"binding_id" => target, "generation" => 5})
      {:ok, page} = Access.decode(:credentials, %{"bindings" => [desired]}, %{"team_id" => "t"})

      assert Access.read_result(unknown, :credentials, {:ok, page}, %{"team_id" => "t"}).receipt.status ==
               :reconciled
    end
  end
end
