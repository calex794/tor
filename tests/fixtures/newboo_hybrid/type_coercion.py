"""Typed coercion helpers for values crossing untyped persistence/API boundaries.

The helpers deliberately keep Python's normal ``int``/``float`` conversion
semantics for supported scalar inputs while making the accepted boundary
explicit to the type checker. Callers should still prefer domain-specific
parsers when a stronger invariant is available.
"""
from __future__ import annotations

from typing import SupportsFloat, SupportsIndex, SupportsInt


def int_from_unknown(value: object, /) -> int:
    """Convert an unknown scalar using ``int`` semantics or fail loudly."""
    if isinstance(value, (str, bytes, bytearray, memoryview)):
        return int(value)
    if isinstance(value, SupportsInt):
        return int(value)
    if isinstance(value, SupportsIndex):
        return int(value)
    raise TypeError(f"value of type {type(value).__name__} cannot be converted to int")


def float_from_unknown(value: object, /) -> float:
    """Convert an unknown scalar using ``float`` semantics or fail loudly."""
    if isinstance(value, (str, bytes, bytearray, memoryview)):
        return float(value)
    if isinstance(value, SupportsFloat):
        return float(value)
    if isinstance(value, SupportsIndex):
        return float(value)
    raise TypeError(f"value of type {type(value).__name__} cannot be converted to float")
