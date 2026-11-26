# RECOVERY-10e-b2: exact S334 encrypted purchase witness

The existing S334 fixture performs two real 175-minor-unit purchases, one
of each encrypted chapter edition. Add a read-only reusable SQL witness to
its existing logical restoration workflow. The pre-backup source snapshot
is retained outside the dump. Compare the exact purchase, wallet, journal,
command, library, posting and publication records after restore. The
complete edition and immutable encrypted registry rows are represented
by hashes rather than plaintext or key metadata. Each checkout has a
425-minor-unit remaining synthetic wallet, 35 platform credit and 140
creator earning; the initial 600 balance does not claim an actual deposit.

The actual entitlement function must grant each pinned version; the
existing post-restore revocation test must make the witness fail with
its specific entitlement error, while leaving the source unchanged.
All statements run in a repeatable-read read-only transaction. No
production data or real payments are involved.

This is a prerequisite for incorporating the same fixture and witness
into the physical WAL recovery acceptance. Schema 7 is unchanged.
Neither physical encrypted paid HTTP/WebCrypto nor independent-host
production recovery is attested by this change alone.
