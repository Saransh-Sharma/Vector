# Computer use

Computer use is a negotiated capability, not a prompt convention. A run must satisfy all of these gates:

1. The chosen harness adapter reports the capability.
2. Effective policy permits inspection or control.
3. The selected isolation substrate is compatible.
4. The OS grants Screen Recording and/or Accessibility permission.
5. A vision model role is present unless the harness supplies a conformant native perception system.
6. `vectord` issues a run-scoped helper grant.

The macOS helper implements the first common protocol substrate. OMP should use its native implementation when conformance proves policy and observation. Pi consumes the helper through a run-scoped extension. DeepSeek remains experimental until its Cordis plugin passes the same tests.

YOLO can auto-approve eligible actions but cannot create OS permissions, escape the run directory, read the clipboard, or weaken a hard deny.

## Verification flow

The Launch Center keeps `computer.inspect` and `computer.control` denied until all checks pass:

1. Select an exact model ID currently loaded in LM Studio for the vision role.
2. Vector opens a local, Vector-owned button fixture and captures it through the separately permissioned helper.
3. The selected model receives the PNG and must return the nonce visible in the pixels. Model names and provider metadata do not count as proof of vision.
4. The helper clicks the isolated target and confirms that its action fired.
5. Only then does Vector atomically assign the vision role, add the computer-use Pack, allow inspection, and set control to prompt for the selected harness profiles.

The screenshot remains beneath Vector's local computer-verification data directory. A changed model role changes the run-spec fingerprint, so the coding smoke test must pass once more before launch.
