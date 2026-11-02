from __future__ import annotations
class DomainError(Exception):
    def __init__(
        self,
        *,
        code: str,
        message: str,
        status_code: int = 400,
        details: dict[str, object] | None = None,
    ) -> None:
        super().__init__(message)
        self.code = str(code).strip()
        self.message = str(message).strip()
        self.status_code = int(status_code)
        self.details = dict(details or {})
    def to_dict(self) -> dict[str, object]:
        return {
            "error": {
                "code": self.code,
                "message": self.message,
                "details": self.details,
            }
        }
class ValidationDomainError(DomainError, ValueError):
    def __init__(self, *, code: str, message: str, details: dict[str, object] | None = None) -> None:
        super().__init__(code=code, message=message, status_code=400, details=details)
class UnauthorizedDomainError(DomainError):
    def __init__(self, *, code: str, message: str, details: dict[str, object] | None = None) -> None:
        super().__init__(code=code, message=message, status_code=401, details=details)
class ForbiddenDomainError(DomainError, ValueError):
    def __init__(self, *, code: str, message: str, details: dict[str, object] | None = None) -> None:
        super().__init__(code=code, message=message, status_code=403, details=details)
class NotFoundDomainError(DomainError, LookupError):
    def __init__(self, *, code: str, message: str, details: dict[str, object] | None = None) -> None:
        super().__init__(code=code, message=message, status_code=404, details=details)
class ConflictDomainError(DomainError, ValueError):
    def __init__(self, *, code: str, message: str, details: dict[str, object] | None = None) -> None:
        super().__init__(code=code, message=message, status_code=409, details=details)

class InvalidIdentifierError(ValueError):
    """Request-facing identifier parsing error that should become HTTP 422."""

    def __init__(self, *, field_name: str, message: str) -> None:
        super().__init__(message)
        self.field_name = str(field_name).strip() or "identifier"
        self.message = str(message).strip() or "Invalid identifier"
