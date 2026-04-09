# TT CLI Reference

_Generated from the current `tt` Clap command tree._

## `tt`

TT v2 local client

**Usage**

```text
Usage: tt [OPTIONS] <COMMAND>
```

**Subcommands**

- `init`
- `open`
- `docs`
- `codex`
- `status`

**Arguments**

- `--cwd` `<CWD>` (optional): Working directory to open the TT runtime in

### `tt init`

**Usage**

```text
Usage: init [OPTIONS]
```

**Arguments**

- `--path` `<PATH>` (optional)
- `--title` `<TITLE>` (optional)
- `--objective` `<OBJECTIVE>` (optional)
- `--template` `<TEMPLATE>` (optional)
- `--base-branch` `<BASE_BRANCH>` (optional)
- `--worktree-root` `<WORKTREE_ROOT>` (optional)
- `--director-model` `<DIRECTOR_MODEL>` (optional)
- `--dev-model` `<DEV_MODEL>` (optional)
- `--test-model` `<TEST_MODEL>` (optional)
- `--integration-model` `<INTEGRATION_MODEL>` (optional)

### `tt open`

**Usage**

```text
Usage: open [OPTIONS]
```

**Arguments**

- `--title` `<TITLE>` (optional)
- `--objective` `<OBJECTIVE>` (optional)
- `--base-branch` `<BASE_BRANCH>` (optional)
- `--worktree-root` `<WORKTREE_ROOT>` (optional)
- `--director-model` `<DIRECTOR_MODEL>` (optional)
- `--dev-model` `<DEV_MODEL>` (optional)
- `--test-model` `<TEST_MODEL>` (optional)
- `--integration-model` `<INTEGRATION_MODEL>` (optional)

### `tt docs`

**Usage**

```text
Usage: docs <COMMAND>
```

**Subcommands**

- `export-cli`

#### `tt docs export-cli`

**Usage**

```text
Usage: export-cli [OPTIONS]
```

**Arguments**

- `--output` `<OUTPUT>` (optional)

### `tt codex`

**Usage**

```text
Usage: codex <COMMAND>
```

**Subcommands**

- `threads`
- `app-servers`

#### `tt codex threads`

**Usage**

```text
Usage: threads <COMMAND>
```

**Subcommands**

- `list`
- `get`
- `read`
- `start`
- `resume`

##### `tt codex threads list`

**Usage**

```text
Usage: list [LIMIT]
```

**Arguments**

- `limit` `<LIMIT>` (optional)

##### `tt codex threads get`

**Usage**

```text
Usage: get <SELECTOR>
```

**Arguments**

- `selector` `<SELECTOR>`

##### `tt codex threads read`

**Usage**

```text
Usage: read [OPTIONS] <SELECTOR>
```

**Arguments**

- `selector` `<SELECTOR>`
- `--include-turns` (optional)

##### `tt codex threads start`

**Usage**

```text
Usage: start [OPTIONS]
```

**Arguments**

- `--model` `<MODEL>` (optional)
- `--ephemeral` (optional)

##### `tt codex threads resume`

**Usage**

```text
Usage: resume [OPTIONS] <SELECTOR>
```

**Arguments**

- `selector` `<SELECTOR>`
- `--model` `<MODEL>` (optional)

#### `tt codex app-servers`

**Usage**

```text
Usage: app-servers
```

### `tt status`

**Usage**

```text
Usage: status
```

