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

