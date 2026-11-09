# architecture and boundaries

choose structure from responsibilities, consumers, and lifetimes. keep architecture proposals revisable as implementation and evidence develop. scale design work to the change; a small edit may only need a quick check of the affected contract.

## ownership and context

read the affected behavior, callers, contracts, and relevant tests before choosing where a change belongs. identify who owns the rule and state, who may change that state, and which consumers depend on its guarantees.

group behavior that must be understood and changed together. separate responsibilities that change independently. share code when its meaning and invariants are shared; similar syntax alone is insufficient.

make the purpose, authority, and lifetime of context explicit. distinguish request or task context, resource lifetime, transaction scope, and runtime representation. a `Context` parameter or shared container still needs clear ownership of the state it carries.

## contracts and dependencies

keep contracts and shared types with the behavior they describe. expose capabilities that consumers need while keeping storage and implementation details private. callers should ask the owner to perform work instead of reproducing its rules.

keep portable behavior independent of platform and runtime adapters. composition connects owners; adapters translate representations. inspect dependency direction before adding an import.

choose the smallest boundary that solves the concrete problem. justify a package or process with a dependency, build, isolation, deployment, or lifetime requirement. keep small helpers local when extraction adds no useful boundary.

keep domain meaning, implementation representation, and transport layout distinct. maintain one authoritative contract across adapters or bindings. alternative implementations must preserve its checks, ordering, failure behavior, identity scope, and effects.

## lifetimes and atomicity

ownership includes creation, publication, replacement, cancellation, eviction, cleanup, and recovery. tie derived state and references to the identity and generation that make them valid. replacement must invalidate or transfer them through their owner.

preserve transaction, lock, admission, and publication boundaries when splitting responsibilities. owners may prepare separate contributions to one coordinated commit; independently successful commits can still violate a shared invariant.

make guarantees across owners and systems explicit. orchestration alone provides no atomicity. preserve recovery behavior for interrupted operations and unconfirmed acknowledgments. each owner defines its cleanup; orchestration coordinates those contributions.

## carrying changes through

carry boundary changes through their actual consumers, including affected tests, fixtures, configuration, and public interfaces. preserve existing behavior unless the task calls for a behavior change. introduce abstractions for concrete consumers and avoid unrelated refactors.

verify the guarantee being changed. source checks establish dependencies; behavioral and runtime checks establish the properties they exercise. use the proportional verification guidance in [development](development.md).
