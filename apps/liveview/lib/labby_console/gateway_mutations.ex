defmodule LabbyConsole.GatewayMutations do
  @moduledoc "Bounded ephemeral Gateway drafts and allowlisted partial mutation requests."
  @derive {Inspect, only: [:mode, :original, :connection, :oauth, :invalid]}
  defstruct [:mode, :original, :baseline, :draft, :connection, oauth: false, invalid: []]

  defmodule Dialog do
    @moduledoc false
    @derive {Inspect, only: [:dialog, :opener, :errors, :test_result]}
    defstruct [
      :model,
      dialog: nil,
      errors: [],
      test_result: nil,
      json_open: false,
      json: "",
      opener: "#gateway-add"
    ]
  end

  @switches ~w(enabled proxy_resources proxy_prompts proxy_skills)
  @patterns ~w(expose_tools expose_resources expose_prompts expose_skills)
  @private ~w(env headers headers_env)
  @text ~w(name display_name url command args bearer_token_env bearer_token_value transport)
  @fields @text ++
            @switches ++
            @patterns ++
            @private ++
            Enum.map(@private, &(&1 <> "_intent")) ++ ~w(display_name_intent)
  @json_fields ~w(name display_name url command args bearer_token_env transport) ++
                 @switches ++ @patterns ++ @private
  @env_name ~r/\A[A-Za-z_][A-Za-z0-9_]*\z/
  @header_name ~r/\A[!#$%&'*+.^_`|~0-9A-Za-z-]+\z/

  def new do
    draft =
      Map.new(@text ++ @private ++ @patterns, &{&1, ""})
      |> Map.merge(%{
        "transport" => "network",
        "args" => "[]",
        "enabled" => "true",
        "proxy_resources" => "true",
        "proxy_prompts" => "true",
        "proxy_skills" => "false",
        "display_name_intent" => "preserve"
      })
      |> Map.merge(Map.new(@private, &{&1 <> "_intent", "preserve"}))

    %__MODULE__{
      mode: :add,
      original: nil,
      baseline: draft,
      draft: draft,
      oauth: false,
      connection: :network,
      invalid: []
    }
  end

  def change(state, params) when is_map(params) do
    selected = Map.take(params, @fields)

    valid = fn value ->
      is_binary(value) and byte_size(value) <= 32_768 and String.valid?(value)
    end

    changes = selected |> Enum.filter(fn {_, value} -> valid.(value) end) |> Map.new()

    invalid =
      (state.invalid -- Map.keys(changes)) ++
        Enum.flat_map(selected, fn {field, value} -> if valid.(value), do: [], else: [field] end)

    %{state | draft: Map.merge(state.draft, changes), invalid: Enum.uniq(invalid)}
  end

  def change(state, _), do: state

  def draft_from_view(data, name) do
    with %{"config" => config, "runtime" => runtime} when is_map(config) and is_map(runtime) <-
           data,
         true <- config["name"] == name and runtime["name"] == name,
         true <- valid_name?(name),
         true <- Enum.all?(@switches ++ ~w(oauth_enabled), &is_boolean(config[&1])),
         true <- is_boolean(runtime["connected"]),
         true <-
           Enum.all?(
             ~w(url command bearer_token_env),
             &(is_nil(config[&1]) or text?(config[&1], 32_768))
           ),
         true <- strings?(config["args"]),
         true <- Enum.all?(@patterns, &(is_nil(config[&1]) or strings?(config[&1]))) do
      state = new()

      connection =
        cond do
          is_binary(config["url"]) and is_nil(config["command"]) -> :network
          is_binary(config["command"]) and is_nil(config["url"]) -> :stdio
          true -> :unknown
        end

      draft =
        state.draft
        |> Map.merge(Map.new(@switches, &{&1, to_string(config[&1])}))
        |> Map.merge(Map.new(~w(name url command bearer_token_env), &{&1, config[&1] || ""}))
        |> Map.put("args", Jason.encode!(config["args"]))
        |> Map.put("transport", "preserved")
        |> Map.merge(
          Map.new(
            @patterns,
            &{&1, if(is_nil(config[&1]), do: "", else: Jason.encode!(config[&1]))}
          )
        )

      {:ok,
       %{
         state
         | mode: :edit,
           original: name,
           baseline: draft,
           draft: draft,
           oauth: config["oauth_enabled"],
           connection: connection
       }}
    else
      _ -> {:error, ["Saved configuration is unavailable or has an unsupported structure."]}
    end
  end

  def build_request(state, :test) when state.mode == :edit,
    do: bounded("gateway.test", %{"name" => state.original})

  def build_request(state, purpose) when purpose in [:save, :test] do
    with {:ok, spec} <- validate_draft(state),
         :ok <- test_credentials(state, purpose) do
      {action, params} = request(state, purpose, spec)

      params =
        if purpose == :save and state.draft["bearer_token_value"] != "",
          do: Map.put(params, "bearer_token_value", state.draft["bearer_token_value"]),
          else: params

      bounded(action, params)
    end
  end

  defp request(_state, :test, spec), do: {"gateway.test", %{"spec" => spec}}
  defp request(%{mode: :add}, :save, spec), do: {"gateway.add", %{"spec" => spec}}

  defp request(state, :save, spec) do
    # The read view is deliberately incomplete. Only explicit changed fields
    # enter the patch. Advanced configuration remains owned by Rust.
    patch =
      Enum.reduce(spec, %{}, fn {field, value}, patch ->
        changed =
          cond do
            field in @private -> state.draft[field <> "_intent"] != "preserve"
            field == "display_name" -> state.draft["display_name_intent"] != "preserve"
            true -> state.draft[field] != state.baseline[field]
          end

        if changed, do: Map.put(patch, field, value), else: patch
      end)

    {"gateway.update", %{"name" => state.original, "patch" => patch}}
  end

  def validate_draft(state) do
    draft = state.draft

    errors =
      if state.invalid == [],
        do: [],
        else: ["A form field exceeds its size limit or has an invalid type."]

    errors =
      check(
        errors,
        valid_name?(draft["name"]),
        "ID must contain 1–128 ASCII letters, digits, dots, underscores or hyphens and use an available namespace."
      )

    errors =
      check(
        errors,
        draft["display_name"] == "" or
          (String.length(draft["display_name"]) <= 80 and not control?(draft["display_name"])),
        "Display name must be at most 80 characters on one line."
      )

    errors =
      check(
        errors,
        Enum.all?(@switches, &(draft[&1] in ~w(true false))),
        "Enable and exposure settings must be true or false."
      )

    errors =
      check(
        errors,
        not (draft["proxy_skills"] == "true") or
          (Regex.match?(~r/\A[a-z0-9](?:[a-z0-9-]*[a-z0-9])?\z/, draft["name"]) and
             draft["name"] != "labby"),
        "Agent Skills require a lowercase ID with interior hyphens."
      )

    errors =
      check(
        errors,
        draft["bearer_token_env"] == "" or Regex.match?(@env_name, draft["bearer_token_env"]),
        "Bearer reference must be an environment variable name."
      )

    errors =
      check(
        errors,
        not state.oauth or
          (draft["bearer_token_env"] == state.baseline["bearer_token_env"] and
             draft["bearer_token_value"] == ""),
        "Existing OAuth settings are preserved. Bearer changes require the OAuth workflow."
      )

    errors =
      check(
        errors,
        state.mode != :edit or draft["transport"] == "preserved",
        "Saved transport metadata is unavailable. Transport changes are unavailable in this form."
      )

    errors =
      check(
        errors,
        state.mode != :edit or draft["bearer_token_value"] == "" or
          draft["bearer_token_env"] != "",
        "A saved server needs a bearer environment reference before replacing its token."
      )

    errors =
      check(
        errors,
        draft["display_name_intent"] in ~w(preserve replace clear),
        "Choose a display name update intent."
      )

    base = Map.new(@switches, &{&1, draft[&1] == "true"}) |> Map.put("name", draft["name"])
    base = Map.put(base, "bearer_token_env", nullable(draft["bearer_token_env"]))

    base =
      if draft["display_name_intent"] != "preserve" or
           (state.mode == :add and draft["display_name"] != ""),
         do:
           Map.put(
             base,
             "display_name",
             if(draft["display_name_intent"] == "clear",
               do: nil,
               else: nullable(draft["display_name"])
             )
           ),
         else: base

    {base, errors} = connection_fields(state, base, errors)

    {base, errors} =
      Enum.reduce(@patterns, {base, errors}, fn field, {spec, errs} ->
        case string_array(draft[field], true) do
          {:ok, values} -> {Map.put(spec, field, values), errs}
          _ -> {spec, errs ++ ["Exposure patterns must be a JSON array of strings or blank."]}
        end
      end)

    {base, errors} =
      Enum.reduce(@private, {base, errors}, fn field, {spec, errs} ->
        case private_map(draft, field) do
          :preserve ->
            {spec, errs}

          {:ok, values} ->
            {Map.put(spec, field, values), errs}

          _ ->
            {spec,
             errs ++
               [
                 "Private configuration requires a valid string-valued JSON object and an explicit update intent."
               ]}
        end
      end)

    names = Map.keys(base["headers"] || %{}) ++ Map.keys(base["headers_env"] || %{})

    errors =
      check(
        errors,
        length(Enum.uniq_by(names, &String.downcase/1)) == length(names),
        "Header names must be unique across both maps, ignoring case."
      )

    if errors == [], do: {:ok, base}, else: {:error, Enum.uniq(errors)}
  end

  defp connection_fields(state, base, errors) do
    draft = state.draft
    connection = if state.mode == :edit, do: state.connection, else: draft["transport"]

    case connection do
      kind when kind in [:network, "network"] ->
        errors =
          check(
            errors,
            valid_url?(draft["url"]),
            "Endpoint must use HTTP, HTTPS, WS or WSS, with a host and no embedded credentials."
          )

        {Map.put(base, "url", draft["url"]), errors}

      kind when kind in [:stdio, "stdio"] ->
        errors =
          check(
            errors,
            String.trim(draft["command"]) != "" and not control?(draft["command"]),
            "Command is required. Enter arguments separately."
          )

        case string_array(draft["args"], false) do
          {:ok, args} ->
            {base |> Map.put("command", draft["command"]) |> Map.put("args", args), errors}

          _ ->
            {base, errors ++ ["Arguments must be a JSON array of strings."]}
        end

      :unknown ->
        errors =
          check(
            errors,
            Enum.all?(~w(url command args), &(draft[&1] == state.baseline[&1])),
            "Connection fields are unavailable for this saved configuration."
          )

        {base, errors}

      _ ->
        {base, errors ++ ["Choose a supported transport."]}
    end
  end

  def apply_json(state, json) when is_binary(json) and byte_size(json) <= 32_768 do
    with {:ok, decoded} <- Jason.decode(json),
         {:ok, spec} <- single_spec(decoded),
         true <- Enum.all?(Map.keys(spec), &(&1 in @json_fields)),
         true <- Enum.all?(@switches, &(not Map.has_key?(spec, &1) or is_boolean(spec[&1]))),
         true <- Enum.all?(@private, &(not Map.has_key?(spec, &1) or string_map?(spec[&1]))),
         true <-
           Enum.all?(
             @patterns,
             &(not Map.has_key?(spec, &1) or is_nil(spec[&1]) or strings?(spec[&1]))
           ),
         true <- not Map.has_key?(spec, "args") or strings?(spec["args"]),
         true <-
           Enum.all?(
             ~w(name display_name url command bearer_token_env),
             &(not Map.has_key?(spec, &1) or is_nil(spec[&1]) or is_binary(spec[&1]))
           ),
         {:ok, transport} <- json_transport(spec),
         true <-
           state.mode == :add or
             ((transport == "network" and state.connection == :network) or
                (transport == "stdio" and state.connection == :stdio)) do
      params =
        Enum.reduce(spec, %{}, fn {key, value}, acc ->
          cond do
            key in @private ->
              acc |> Map.put(key, Jason.encode!(value)) |> Map.put(key <> "_intent", "replace")

            key in @patterns or key == "args" ->
              Map.put(acc, key, if(is_nil(value), do: "", else: Jason.encode!(value)))

            key in @switches ->
              Map.put(acc, key, to_string(value))

            key == "transport" ->
              acc

            true ->
              Map.put(acc, key, value || "")
          end
        end)
        |> Map.put("transport", if(state.mode == :add, do: transport, else: "preserved"))

      params =
        if Map.has_key?(spec, "display_name"),
          do:
            Map.put(
              params,
              "display_name_intent",
              if(is_nil(spec["display_name"]), do: "clear", else: "replace")
            ),
          else: params

      updated = change(state, params)

      case validate_draft(updated) do
        {:ok, _} -> {:ok, updated}
        {:error, errors} -> {:error, errors}
      end
    else
      _ ->
        {:error,
         [
           "JSON must contain exactly one supported server with correctly typed fields. Unsupported fields require their owning workflow."
         ]}
    end
  end

  def apply_json(_, _), do: {:error, ["JSON exceeds the 32 KiB draft limit."]}

  defp single_spec(%{"mcpServers" => servers} = wrapper) when map_size(wrapper) == 1,
    do: named_spec(servers)

  defp single_spec(%{"name" => name} = spec) when is_binary(name), do: {:ok, spec}
  defp single_spec(servers), do: named_spec(servers)

  defp named_spec(servers) when is_map(servers) and map_size(servers) == 1 do
    [{name, spec}] = Map.to_list(servers)

    if is_map(spec) and (not Map.has_key?(spec, "name") or spec["name"] == name),
      do: {:ok, Map.put(spec, "name", name)},
      else: :error
  end

  defp named_spec(_), do: :error

  defp json_transport(spec) do
    cond do
      is_binary(spec["url"]) and is_nil(spec["command"]) and spec["args"] in [nil, []] and
        spec["transport"] in [nil, "http", "websocket"] and matching_url_transport?(spec) ->
        {:ok, "network"}

      is_binary(spec["command"]) and is_nil(spec["url"]) and spec["transport"] in [nil, "stdio"] ->
        {:ok, "stdio"}

      true ->
        :error
    end
  end

  defp matching_url_transport?(spec) do
    scheme = URI.parse(spec["url"]).scheme

    case spec["transport"] do
      "http" -> scheme in ~w(http https)
      "websocket" -> scheme in ~w(ws wss)
      nil -> true
    end
  end

  def test_result(
        %{
          "name" => name,
          "connected" => true,
          "last_error" => nil,
          "capability_observation" => observation
        },
        name
      )
      when is_map(observation) do
    known =
      Enum.any?(~w(tools resources prompts skills), fn family ->
        case observation[family] do
          %{"state" => "known", "discovered" => d, "exposed" => e}
          when is_integer(d) and d >= 0 and is_integer(e) and e >= 0 ->
            true

          _ ->
            false
        end
      end)

    if known,
      do: {:ok, %{connected: true}},
      else: {:error, "Probe completed without a verified capability observation."}
  end

  def test_result(_, _),
    do: {:error, "Probe did not establish a verified connection. Provider details are hidden."}

  def valid_name?(name),
    do:
      text?(name, 128) and Regex.match?(~r/\A[A-Za-z0-9_.-]+\z/, name) and
        not String.starts_with?(name, "__in_process__")

  defp test_credentials(%{draft: draft}, :test) do
    if draft["bearer_token_value"] == "",
      do: :ok,
      else:
        {:error,
         [
           "Draft probes cannot use a pasted bearer token. Save explicitly or use an environment reference."
         ]}
  end

  defp test_credentials(_, _), do: :ok

  defp bounded(action, params) do
    case Jason.encode(%{action: action, params: params}) do
      {:ok, encoded} when byte_size(encoded) <= 32_768 -> {:ok, action, params}
      _ -> {:error, ["The encoded request exceeds the 32 KiB limit."]}
    end
  end

  defp private_map(draft, field) do
    case draft[field <> "_intent"] do
      "preserve" ->
        :preserve

      "clear" ->
        {:ok, %{}}

      "replace" ->
        with {:ok, values} <- Jason.decode(draft[field]),
             true <- string_map?(values),
             true <- valid_map?(field, values),
             do: {:ok, values},
             else: (_ -> :error)

      _ ->
        :error
    end
  end

  defp valid_map?("env", _values), do: true

  defp valid_map?(field, values),
    do:
      Enum.all?(values, fn {key, value} ->
        Regex.match?(@header_name, key) and String.downcase(key) != "authorization" and
          not control?(value) and (field != "headers_env" or Regex.match?(@env_name, value))
      end)

  defp string_map?(values),
    do:
      is_map(values) and map_size(values) <= 128 and
        Enum.all?(values, fn {k, v} -> text?(k, 1024) and text?(v, 32_768) end)

  defp string_array("", true), do: {:ok, nil}

  defp string_array(text, _) do
    case Jason.decode(text) do
      {:ok, values} -> if strings?(values), do: {:ok, values}, else: :error
      _ -> :error
    end
  end

  defp strings?(values),
    do: is_list(values) and length(values) <= 256 and Enum.all?(values, &text?(&1, 4096))

  defp text?(value, max),
    do: is_binary(value) and byte_size(value) <= max and String.valid?(value)

  defp nullable(""), do: nil
  defp nullable(value), do: value
  defp control?(value), do: Regex.match?(~r/[\x00-\x1f\x7f]/, value)

  defp valid_url?(value) do
    case URI.new(value) do
      {:ok, %URI{scheme: scheme, host: host, userinfo: nil}} ->
        scheme in ~w(http https ws wss) and is_binary(host) and host != "" and not control?(value)

      _ ->
        false
    end
  end

  defp check(errors, true, _), do: errors
  defp check(errors, _, message), do: errors ++ [message]
end
