# REUSE-09A — canonical rank/report common SQL

C/F read-only projection. Base: 2d4188ff63efd9ca52f6a17775511028254170a6.

Both complete query texts are preserved exactly, including whitespace and final
newlines. The common prefix is 63 lines (3500 ASCII bytes), ending with
the SELECT inside the selected CTE. Only that shared prefix is extracted;
ranking and keyset pagination retain their separate tails and bind order.
Rust concat! + include_str! assemble static &'static str at compile time,
without allocations or a build script. No migrations, grants, new tables,
write paths, transaction isolation or timeouts change.

Original Git blob witnesses: rank a3527812f7491389196b1be3ec1c9d279ff1a3e1 (4175 bytes);
report 29f1f0ee44727dff1a4558ba8599fd0b17ce7e18 (4290 bytes). Both reconstructed
strings were compared to the original source before committing. The existing
architecture test now reconstructs both from bytes and checks the reviewed
blob IDs, as well as actual call sites. The original report source guard
and PG boundary test are preserved, adapting their source assembly only.
Future intentional SQL changes require updating the reviewed witnesses.

Acceptance: exact-head Repository rules, Rust, PG audience canonical
report/rank (including lock recovery, firsts, taxonomy and 100k), browser
and full Integration CI, then post-merge. No deployment or live data.
