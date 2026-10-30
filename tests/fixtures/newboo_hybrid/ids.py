from __future__ import annotations
from typing import NewType, TypeAlias
from uuid import UUID
from app.core.errors import InvalidIdentifierError
from app.core.numeric_bounds import YDB_INT64_MAX
UserId = NewType("UserId", int)
BookId = NewType("BookId", int)
PublicBookId = NewType("PublicBookId", str)
PrivateBookId = NewType("PrivateBookId", str)
PrivateChapterId = NewType("PrivateChapterId", str)
BookReference: TypeAlias = BookId | PublicBookId | UUID
ChapterId = NewType("ChapterId", int)
LayerId = NewType("LayerId", int)
LayerAccessId = NewType("LayerAccessId", int)
LayerVersionId = NewType("LayerVersionId", int)
TeamId = NewType("TeamId", int)
PublicTeamId = NewType("PublicTeamId", str)
TeamReference: TypeAlias = TeamId | PublicTeamId | UUID
InvitationId = NewType("InvitationId", int)
PurchaseId = NewType("PurchaseId", str)
RefundId = NewType("RefundId", str)
def _to_positive_int_id(value: object, *, name: str) -> int:
    if value is None:
        raise ValueError(f"{name} пустой")
    clean = str(value).strip()
    if not clean:
        raise ValueError(f"{name} пустой")
    try:
        parsed = int(clean)
    except (TypeError, ValueError) as exc:
        raise ValueError(f"{name} должен быть числом") from exc
    if parsed <= 0:
        raise ValueError(f"{name} должен быть больше нуля")
    if parsed > YDB_INT64_MAX:
        raise ValueError(f"{name} превышает диапазон YDB Int64")
    return parsed
def _to_non_empty_str_id(value: object, *, name: str) -> str:
    clean = str(value or "").strip()
    if not clean:
        raise ValueError(f"{name} пустой")
    return clean
def to_user_id(value: object) -> UserId:
    return UserId(_to_positive_int_id(value, name="user_id"))
def to_book_id(value: object) -> BookId:
    return BookId(_to_positive_int_id(value, name="book_id"))
def to_book_reference(value: object) -> BookReference:
    if isinstance(value, UUID):
        return value
    if isinstance(value, bool) or isinstance(value, float):
        raise InvalidIdentifierError(
            field_name="book_id",
            message="book_id должен быть корректным идентификатором",
        )
    if isinstance(value, int):
        try:
            return to_book_id(value)
        except ValueError as exc:
            raise InvalidIdentifierError(field_name="book_id", message=str(exc)) from exc
    if not isinstance(value, str):
        raise InvalidIdentifierError(
            field_name="book_id",
            message="book_id должен быть строкой, целым числом или UUID",
        )
    try:
        clean = _to_non_empty_str_id(value, name="book_id")
    except ValueError as exc:
        raise InvalidIdentifierError(field_name="book_id", message=str(exc)) from exc
    if clean.lstrip("+-").isdigit():
        try:
            return to_book_id(clean)
        except ValueError as exc:
            raise InvalidIdentifierError(field_name="book_id", message=str(exc)) from exc
    try:
        return UUID(clean)
    except ValueError:
        return PublicBookId(clean)
def to_public_book_id(value: object) -> PublicBookId:
    return PublicBookId(_to_non_empty_str_id(value, name="public_book_id"))
def to_private_book_id(value: object) -> PrivateBookId:
    return PrivateBookId(_to_non_empty_str_id(value, name="private_book_id"))
def to_private_chapter_id(value: object) -> PrivateChapterId:
    return PrivateChapterId(_to_non_empty_str_id(value, name="private_chapter_id"))
def to_chapter_id(value: object) -> ChapterId:
    if isinstance(value, bool) or isinstance(value, float):
        raise InvalidIdentifierError(
            field_name="chapter_id",
            message="chapter_id должен быть корректным идентификатором",
        )
    try:
        return ChapterId(_to_positive_int_id(value, name="chapter_id"))
    except ValueError as exc:
        raise InvalidIdentifierError(field_name="chapter_id", message=str(exc)) from exc
def to_layer_id(value: object) -> LayerId:
    return LayerId(_to_positive_int_id(value, name="layer_id"))
def to_layer_access_id(value: object) -> LayerAccessId:
    return LayerAccessId(_to_positive_int_id(value, name="layer_access_id"))
def to_layer_version_id(value: object) -> LayerVersionId:
    return LayerVersionId(_to_positive_int_id(value, name="layer_version_id"))
def to_team_id(value: object) -> TeamId:
    return TeamId(_to_positive_int_id(value, name="team_id"))
def to_public_team_id(value: object) -> PublicTeamId:
    return PublicTeamId(_to_non_empty_str_id(value, name="public_team_id"))
def to_team_reference(value: object) -> TeamReference:
    if isinstance(value, UUID):
        return value
    clean = _to_non_empty_str_id(value, name="team_id")
    if clean.lstrip("+-").isdigit():
        return to_team_id(clean)
    try:
        return UUID(clean)
    except ValueError:
        return PublicTeamId(clean)
def to_invitation_id(value: object) -> InvitationId:
    return InvitationId(_to_positive_int_id(value, name="invitation_id"))
def to_purchase_id(value: object) -> PurchaseId:
    try:
        return PurchaseId(_to_non_empty_str_id(value, name="purchase_id"))
    except ValueError as exc:
        raise InvalidIdentifierError(field_name="purchase_id", message=str(exc)) from exc
def to_refund_id(value: object) -> RefundId:
    try:
        return RefundId(_to_non_empty_str_id(value, name="refund_id"))
    except ValueError as exc:
        raise InvalidIdentifierError(field_name="refund_id", message=str(exc)) from exc
