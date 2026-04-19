# Performance Observability

This specification covers tracing and profiling expectations for the discovery pipeline in `locate-git-projects-on-my-computer`.

tool[profiling.discovery-phases-spanned]
The discovery implementation must emit coarse tracing spans around indexed candidate lookup, enrichment scheduling and collection, and result merge so a Tracy capture explains time to completion.

tool[profiling.hot-loop-spans-tracy-gated]
High-volume per-repository or per-chunk tracing spans in the discovery pipeline should only be enabled in `tracy` builds.

tool[profiling.discovery-bounded-fields]
The coarse discovery spans should expose bounded fields such as concurrency and author-scan budget settings so captures can be compared across runs.
