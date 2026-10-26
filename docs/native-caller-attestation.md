# Caller attestation after native moves

Superseded by [observe native state and test behaviour under outside changes](native-observe-and-act.md). Do not implement this proposal or extend its proof types.

The earlier design separated request origin from native location. It proposed a Herdr-backed caller witness, or continuity through an already-attested Pi child. A child restarted after a move could not recover origin from its inherited pane address, even when Herdr reported the same conversation.

The replacement trusts the Pi child's bound Session identity and uses current native observations to locate it. Safe effect targeting belongs at the frontend boundary; repeated caller attestation cannot prevent an address changing between observation and action. Missing or ambiguous native state stops only the affected native operation.

`PiCallerObservation` and the strict inherited-pane checks remain in the implementation until that replacement lands. Their presence is not a requirement for the new design.
