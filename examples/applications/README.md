# Example applications

Small, complete projects built on RUA. Each one has been run on a physical
SO-101. Read its README before running it: these move a real arm.

They share the dependencies of the [Python examples](../python/README.md).
From the checkout root:

```sh
uv sync --project examples/python --locked
```

| Application | What it does | Uses |
|---|---|---|
| [Block oracle](block-oracle/README.md) | Ask a yes/no question about blocks on the table. The model inspects the scene with the wrist camera, then the arm nods or shakes its head. | LLM-driven inspection, scripted gestures, run logging |
