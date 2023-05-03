# Version Control

[![Rust](https://github.com/SamuelSchlesinger/version-control/actions/workflows/rust.yml/badge.svg)](https://github.com/SamuelSchlesinger/version-control/actions/workflows/rust.yml)

An experiment to write a version control system from scratch in Rust.

## CLI Usage

```
Usage: revtool <COMMAND>

Commands:
  init      initialize a brand new revision
  diff      check the difference between this branch and another
  changes   shows the files and directories which have been changed since the latest snap
  snap      take a new snapshot
  checkout  switch to branch
  branch    print out current branch
  help      Print this message or the help of the given subcommand(s)

Options:
  -h, --help  Print help
```

## How Does It Work?

Under the hood, it is using a content addressed binary object store in
`.rev/store` using the [BLAKE3](https://github.com/BLAKE3-team/BLAKE3) hash
function to identify binary objects using a type called `ObjectId`. Building on
top of that, we have a `Directory` data type which represents a directory tree
structure where each file's name is mapped to it's `ObjectId`. We store this
type directly, encoded as prettified JSON, in the object store as a commitment
to a particular configuration or version, then we have a data type called
`SnapShot` which links these together into a directed, acyclic graph with each
vertex having a message attached:

```
pub struct SnapShot {
  message: String,
  directory: ObjectId,
  previous: ObjectId,
}
```

We then store this type directly in the object store as well, again as
prettified JSON. In the `.rev/branches` directory, we keep a file for each
branch with the `ObjectId` of a particular encoded `SnapShot`. In
`.rev/branch`, we keep the name of the current branch we're using.

Finally, when we construct a `Directory` from the current directory, often we
don't care about many files, so we have an ignore list in `.rev/ignores` which
configures which paths we ignore.

## Contributing

There are a number of issues on the GitHub repository, please feel free to take
any and do them.
