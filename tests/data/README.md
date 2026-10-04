# Test fixtures

These `.mod` files are fixtures for the loader and writer. They are freely licensed songs, plus one OpenMPT loader test. They are not a demo playlist, and copyrighted modules do not belong here.

The license is not the same for every file:

- **CC0 1.0** — `fx-poly1_k0wax_CC0.mod`, `k0w-rsnd_k0wax_CC0.mod`
- **Public domain** — `10kdub_JAM_PD.mod`
- **CC BY 4.0** (attribution required) — `11thhour_TDK_CCBY.mod`, `dog2_songerson_CCBY.mod`, `momentary_meditation_kjose_CCBY.mod`, `mule10k_newtined_CCBY.mod`, `particle_man_potajoe_CCBY.mod`, `tiny_chip_m4kachu_CCBY.mod`
- **BSD-3-Clause** — `openmpt_test_BSD3.mod`, OpenMPT's loader test (`test/test.mod`). The license text is `OPENMPT_LICENSE_BSD3.txt`.

Artist, title, license URL, source page, and sha256 for every file are in `ATTRIBUTION.txt`.

`tests/sample_fixtures.rs` parses each module and checks that writing it back matches the file bytes. Every file in this set, including the OpenMPT loader test, is an ordinary 31-sample `M.K.` module, so a byte-identical round trip is required. A future fixture that this format cannot store losslessly should assert a successful parse or the specific error, and the test should say why.
