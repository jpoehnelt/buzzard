# State machine fixtures

These files come from three public repositories. The contents are
unmodified, and only the file names differ from upstream. The state machine
extraction tests in `crates/buzzard-lint/tests/verify.rs` read the files.
So do tests of the block diagram decoder in
`crates/buzzard-rsrc/tests/conformance.rs`. Each directory holds the license
of the repository that its files come from.

| File | Source | License | What it is |
| --- | --- | --- | --- |
| `jki-smo/process.vi` | [JKISoftware/JKI-State-Machine-Objects](https://github.com/JKISoftware/JKI-State-Machine-Objects), `src/SMO/Process.vi` at `d496f05` | BSD 3-Clause, `jki-smo/LICENSE` | A classic state machine. It has a while loop, a string shift register, and a case structure with the cases `"Init"` and `"Idle", Default` |
| `jki-state-machine/queued-state-machine-template.vi` | [JKISoftware/JKI-State-Machine](https://github.com/JKISoftware/JKI-State-Machine), `source/State Machine Templates/String-Based Queued State Machine (Basic 1.0).vi` at `df4a757` | BSD 3-Clause, `jki-state-machine/LICENSE` | A string/queue state machine in which every enqueued name has a case |
| `jki-state-machine/new-support-vis-examples.vi` | same repository, `docs/new support VIs 2020/New Support VIs Examples.vi` at `df4a757` | BSD 3-Clause, `jki-state-machine/LICENSE` | A string/queue state machine that enqueues three names without a case |
| `jki-state-machine/parse-state-queue-performance-test.vi` | same repository, `tests/Performance/Parse State Queue Performance Test.vi` at `df4a757` | BSD 3-Clause, `jki-state-machine/LICENSE` | Not a state machine. The loop counter selects the case of a case structure in a while loop |
| `lv-mqtt-broker/test-simple-connection.vi` | [LabVIEW-Open-Source/LV-MQTT-Broker](https://github.com/LabVIEW-Open-Source/LV-MQTT-Broker), `Unit Tests/Manual Dev Tests/Test Simple Connection.vi` at `01aa784` | Zero-Clause BSD, `lv-mqtt-broker/LICENSE` | Not a state machine. An error cluster from a shift register selects the case of a case structure in a while loop |

## Classic enumeration machines are not here

Seven public repositories hold classic state machines with an enumeration
selector: `lallers/StateMachine`, `Gluvertski1/StateMachine_LabVIEW`,
`GaborDernoczi/LabVIEW_State_Machine`, `AhmedDMoHassan/Simple-State-machine`,
`TomsLabVIEWAdventure/Guessing-Game`,
`frank1641/Multiple-consumer-State-machine`, and
`CandleScience/Labview-State-Machine-Template`. None of the seven has a
license file, so this repository holds none of their files. The tests build
the same shapes as `BlockDiagram` values.

Two tests in `crates/buzzard-lint/tests/verify.rs` read the real files. Both
tests are ignored by default. They need `BUZZARD_FSM_SAMPLES` to name a
directory that holds clones of the seven repositories, each named
`<owner>_<repository>`.

`real_machines_outside_the_repository` checks the exact machine that the
extraction gives for each file.
`damage_to_the_real_machines_proves_nothing_new` changes single bytes of
each file's inflated block diagram heap. That test checks that no proven
finding appears that the untouched file does not have.
