# Contract alignment — uncompiled and unverified

This exact replacement updates the retained full H contract after the four-fix,
receipt and atomic-handoff source proposals. Apply `../contract-fix.json` after
the earlier patches; the original checkpoint document remains unchanged.

The authoritative candidate now describes open-ended unresolved epochs and
their retention exception, durable startup gaps, optional legacy capability,
two-sided clock uncertainty, conditional FLOW receipt, and atomic predecessor
replacement. Terminal retire explicitly requires HTTP to stop and drain.
The real flow_runtime order has not been verified.

The former generic test-success prose is replaced with the proposed acceptance
cases and an explicit statement that this new tree has not been compiled or
executed. Earlier local results belong to a different tree.

See `../INTEGRATION.md` for the complete restore sequence, latest candidate map,
scope limits, static review evidence and the remaining real acceptance gates.
