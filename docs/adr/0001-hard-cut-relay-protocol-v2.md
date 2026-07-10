# Hard-cut to Relay Protocol v2

Replace Relay Protocol v1's separate room and admission values with one
high-entropy Session Credential, and ship the change as an intentionally
incompatible Relay Protocol v2 plus a new Join Code version. The project is in
active testing, so accepting a coordinated Relay and Client Bundle rollout is
preferable to retaining a temporary v1 adapter that would preserve the rejected
domain model. V2 must add a real compatibility boundary for
[issue #6](https://github.com/mcthesw/TractorBeam/issues/6), keep negotiation off
the game-packet hot path, and prioritize latency, maintainability, and usability.
End-to-end AEAD and game-payload confidentiality are intentionally not v2 goals.
V2 also does not reserve cryptographic fields for them; any future encrypted
design requires a separately justified Relay Protocol v3.
