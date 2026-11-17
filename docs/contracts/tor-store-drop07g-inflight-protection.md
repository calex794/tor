# DROP-07g — protect in-flight object identities during inventory

S3 may contain bytes after upload and before the PostgreSQL publication or
stage receipt commits. The observed inventory now checks reserved author
and import SINGLE identities even when the final receipt is absent. It also
treats canonical direct-PACK and manifest names under a book's import prefix
as protected while a queued or running S023 job has committed PACK
transport selection. Until there is a verified receipt, these reservations
do not identify a bucket, so protection errs on the side of retaining data.

The existing registered and staged states keep precedence. The accepted
registries include historical placements and preserved FREE V1. The PG18
regression covers the author window before publication, the direct PACK
window before stage proof, and an unrelated object that stays UNKNOWN.

The report remains advisory. It carries no proof of a quiescent writer set,
external backups, a closed recovery window, or authority to remove data.
Physical garbage collection must fence exact identities against future
publication and confirm version-bound provider operations.
