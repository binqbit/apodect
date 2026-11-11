# Documentation

Start with the product, then use the development guides:

- [Concept](concept.md): who Apodect serves and where its control ends.
- [Architecture](architecture.md): coordination, local custody, delivery and evidence.
- [How it works](how-it-works.md): requests, budgets, task execution and recovery.
- [Contract rules](contracts.md): shared accounting and lifecycle behavior.
- [Application development](application-development.md): one-command Compose setup with React, Rust, PostgreSQL, Kafka and local chains; live deployment.
- [Backend services](backend.md): wallet profiles, unsigned actions, indexing, liquidity and durable worker execution.
- [Native local development](local-development.md): host toolchains, contract checks and local chain reset.
- [Local workflows](cross-chain-development.md): create tasks, fund Projects and exercise failures.
- [Coordination protocol](cross-chain-protocol.md): identity, authorization and settlement guarantees.
- [Provider integration](cross-chain-providers.md): simulated boundaries and public deployment requirements.

The local stack runs real contracts and test-token transfers. MPC and liquidity
providers are simulated; a successful local run does not establish public provider
availability or a public deployment. Package READMEs cover chain-specific setup;
exact types and method signatures live with the implementation.
