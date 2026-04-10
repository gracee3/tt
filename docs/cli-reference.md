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

### `tt status`

**Usage**

```text
Usage: status
```

