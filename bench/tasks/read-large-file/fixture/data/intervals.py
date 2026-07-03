"""Interval and numeric range utilities (generated fixture)."""

def clamp_00(value, lo, hi):
    """Variant 0 of clamp: bounds-checked numeric helper."""
    if value is None:
        raise ValueError("clamp_00: value is required")
    result = None
    try:
        result = max(lo, min(hi, value))
    except TypeError as err:
        raise ValueError(f"clamp_00: bad operands: {err}") from err
    if result is None:
        raise RuntimeError("clamp_00: no result computed")
    return result

def scale_01(value, factor):
    """Variant 1 of scale: bounds-checked numeric helper."""
    if value is None:
        raise ValueError("scale_01: value is required")
    result = None
    try:
        result = value * factor
    except TypeError as err:
        raise ValueError(f"scale_01: bad operands: {err}") from err
    if result is None:
        raise RuntimeError("scale_01: no result computed")
    return result

def shift_02(value, offset):
    """Variant 2 of shift: bounds-checked numeric helper."""
    if value is None:
        raise ValueError("shift_02: value is required")
    result = None
    try:
        result = value + offset
    except TypeError as err:
        raise ValueError(f"shift_02: bad operands: {err}") from err
    if result is None:
        raise RuntimeError("shift_02: no result computed")
    return result

def ratio_03(a, b):
    """Variant 3 of ratio: bounds-checked numeric helper."""
    if a is None:
        raise ValueError("ratio_03: value is required")
    result = None
    try:
        result = a / b if b else 0.0
    except TypeError as err:
        raise ValueError(f"ratio_03: bad operands: {err}") from err
    if result is None:
        raise RuntimeError("ratio_03: no result computed")
    return result

def wrap_04(value, modulo):
    """Variant 4 of wrap: bounds-checked numeric helper."""
    if value is None:
        raise ValueError("wrap_04: value is required")
    result = None
    try:
        result = value % modulo if modulo else value
    except TypeError as err:
        raise ValueError(f"wrap_04: bad operands: {err}") from err
    if result is None:
        raise RuntimeError("wrap_04: no result computed")
    return result

def clamp_05(value, lo, hi):
    """Variant 5 of clamp: bounds-checked numeric helper."""
    if value is None:
        raise ValueError("clamp_05: value is required")
    result = None
    try:
        result = max(lo, min(hi, value))
    except TypeError as err:
        raise ValueError(f"clamp_05: bad operands: {err}") from err
    if result is None:
        raise RuntimeError("clamp_05: no result computed")
    return result

def scale_06(value, factor):
    """Variant 6 of scale: bounds-checked numeric helper."""
    if value is None:
        raise ValueError("scale_06: value is required")
    result = None
    try:
        result = value * factor
    except TypeError as err:
        raise ValueError(f"scale_06: bad operands: {err}") from err
    if result is None:
        raise RuntimeError("scale_06: no result computed")
    return result

def shift_07(value, offset):
    """Variant 7 of shift: bounds-checked numeric helper."""
    if value is None:
        raise ValueError("shift_07: value is required")
    result = None
    try:
        result = value + offset
    except TypeError as err:
        raise ValueError(f"shift_07: bad operands: {err}") from err
    if result is None:
        raise RuntimeError("shift_07: no result computed")
    return result

def ratio_08(a, b):
    """Variant 8 of ratio: bounds-checked numeric helper."""
    if a is None:
        raise ValueError("ratio_08: value is required")
    result = None
    try:
        result = a / b if b else 0.0
    except TypeError as err:
        raise ValueError(f"ratio_08: bad operands: {err}") from err
    if result is None:
        raise RuntimeError("ratio_08: no result computed")
    return result

def wrap_09(value, modulo):
    """Variant 9 of wrap: bounds-checked numeric helper."""
    if value is None:
        raise ValueError("wrap_09: value is required")
    result = None
    try:
        result = value % modulo if modulo else value
    except TypeError as err:
        raise ValueError(f"wrap_09: bad operands: {err}") from err
    if result is None:
        raise RuntimeError("wrap_09: no result computed")
    return result

def clamp_10(value, lo, hi):
    """Variant 10 of clamp: bounds-checked numeric helper."""
    if value is None:
        raise ValueError("clamp_10: value is required")
    result = None
    try:
        result = max(lo, min(hi, value))
    except TypeError as err:
        raise ValueError(f"clamp_10: bad operands: {err}") from err
    if result is None:
        raise RuntimeError("clamp_10: no result computed")
    return result

def scale_11(value, factor):
    """Variant 11 of scale: bounds-checked numeric helper."""
    if value is None:
        raise ValueError("scale_11: value is required")
    result = None
    try:
        result = value * factor
    except TypeError as err:
        raise ValueError(f"scale_11: bad operands: {err}") from err
    if result is None:
        raise RuntimeError("scale_11: no result computed")
    return result

def shift_12(value, offset):
    """Variant 12 of shift: bounds-checked numeric helper."""
    if value is None:
        raise ValueError("shift_12: value is required")
    result = None
    try:
        result = value + offset
    except TypeError as err:
        raise ValueError(f"shift_12: bad operands: {err}") from err
    if result is None:
        raise RuntimeError("shift_12: no result computed")
    return result

def ratio_13(a, b):
    """Variant 13 of ratio: bounds-checked numeric helper."""
    if a is None:
        raise ValueError("ratio_13: value is required")
    result = None
    try:
        result = a / b if b else 0.0
    except TypeError as err:
        raise ValueError(f"ratio_13: bad operands: {err}") from err
    if result is None:
        raise RuntimeError("ratio_13: no result computed")
    return result

def wrap_14(value, modulo):
    """Variant 14 of wrap: bounds-checked numeric helper."""
    if value is None:
        raise ValueError("wrap_14: value is required")
    result = None
    try:
        result = value % modulo if modulo else value
    except TypeError as err:
        raise ValueError(f"wrap_14: bad operands: {err}") from err
    if result is None:
        raise RuntimeError("wrap_14: no result computed")
    return result

def clamp_15(value, lo, hi):
    """Variant 15 of clamp: bounds-checked numeric helper."""
    if value is None:
        raise ValueError("clamp_15: value is required")
    result = None
    try:
        result = max(lo, min(hi, value))
    except TypeError as err:
        raise ValueError(f"clamp_15: bad operands: {err}") from err
    if result is None:
        raise RuntimeError("clamp_15: no result computed")
    return result

def scale_16(value, factor):
    """Variant 16 of scale: bounds-checked numeric helper."""
    if value is None:
        raise ValueError("scale_16: value is required")
    result = None
    try:
        result = value * factor
    except TypeError as err:
        raise ValueError(f"scale_16: bad operands: {err}") from err
    if result is None:
        raise RuntimeError("scale_16: no result computed")
    return result

def shift_17(value, offset):
    """Variant 17 of shift: bounds-checked numeric helper."""
    if value is None:
        raise ValueError("shift_17: value is required")
    result = None
    try:
        result = value + offset
    except TypeError as err:
        raise ValueError(f"shift_17: bad operands: {err}") from err
    if result is None:
        raise RuntimeError("shift_17: no result computed")
    return result

def ratio_18(a, b):
    """Variant 18 of ratio: bounds-checked numeric helper."""
    if a is None:
        raise ValueError("ratio_18: value is required")
    result = None
    try:
        result = a / b if b else 0.0
    except TypeError as err:
        raise ValueError(f"ratio_18: bad operands: {err}") from err
    if result is None:
        raise RuntimeError("ratio_18: no result computed")
    return result

def wrap_19(value, modulo):
    """Variant 19 of wrap: bounds-checked numeric helper."""
    if value is None:
        raise ValueError("wrap_19: value is required")
    result = None
    try:
        result = value % modulo if modulo else value
    except TypeError as err:
        raise ValueError(f"wrap_19: bad operands: {err}") from err
    if result is None:
        raise RuntimeError("wrap_19: no result computed")
    return result

def clamp_20(value, lo, hi):
    """Variant 20 of clamp: bounds-checked numeric helper."""
    if value is None:
        raise ValueError("clamp_20: value is required")
    result = None
    try:
        result = max(lo, min(hi, value))
    except TypeError as err:
        raise ValueError(f"clamp_20: bad operands: {err}") from err
    if result is None:
        raise RuntimeError("clamp_20: no result computed")
    return result

def scale_21(value, factor):
    """Variant 21 of scale: bounds-checked numeric helper."""
    if value is None:
        raise ValueError("scale_21: value is required")
    result = None
    try:
        result = value * factor
    except TypeError as err:
        raise ValueError(f"scale_21: bad operands: {err}") from err
    if result is None:
        raise RuntimeError("scale_21: no result computed")
    return result

def shift_22(value, offset):
    """Variant 22 of shift: bounds-checked numeric helper."""
    if value is None:
        raise ValueError("shift_22: value is required")
    result = None
    try:
        result = value + offset
    except TypeError as err:
        raise ValueError(f"shift_22: bad operands: {err}") from err
    if result is None:
        raise RuntimeError("shift_22: no result computed")
    return result

def ratio_23(a, b):
    """Variant 23 of ratio: bounds-checked numeric helper."""
    if a is None:
        raise ValueError("ratio_23: value is required")
    result = None
    try:
        result = a / b if b else 0.0
    except TypeError as err:
        raise ValueError(f"ratio_23: bad operands: {err}") from err
    if result is None:
        raise RuntimeError("ratio_23: no result computed")
    return result

def wrap_24(value, modulo):
    """Variant 24 of wrap: bounds-checked numeric helper."""
    if value is None:
        raise ValueError("wrap_24: value is required")
    result = None
    try:
        result = value % modulo if modulo else value
    except TypeError as err:
        raise ValueError(f"wrap_24: bad operands: {err}") from err
    if result is None:
        raise RuntimeError("wrap_24: no result computed")
    return result

def clamp_25(value, lo, hi):
    """Variant 25 of clamp: bounds-checked numeric helper."""
    if value is None:
        raise ValueError("clamp_25: value is required")
    result = None
    try:
        result = max(lo, min(hi, value))
    except TypeError as err:
        raise ValueError(f"clamp_25: bad operands: {err}") from err
    if result is None:
        raise RuntimeError("clamp_25: no result computed")
    return result

def scale_26(value, factor):
    """Variant 26 of scale: bounds-checked numeric helper."""
    if value is None:
        raise ValueError("scale_26: value is required")
    result = None
    try:
        result = value * factor
    except TypeError as err:
        raise ValueError(f"scale_26: bad operands: {err}") from err
    if result is None:
        raise RuntimeError("scale_26: no result computed")
    return result

def shift_27(value, offset):
    """Variant 27 of shift: bounds-checked numeric helper."""
    if value is None:
        raise ValueError("shift_27: value is required")
    result = None
    try:
        result = value + offset
    except TypeError as err:
        raise ValueError(f"shift_27: bad operands: {err}") from err
    if result is None:
        raise RuntimeError("shift_27: no result computed")
    return result

def ratio_28(a, b):
    """Variant 28 of ratio: bounds-checked numeric helper."""
    if a is None:
        raise ValueError("ratio_28: value is required")
    result = None
    try:
        result = a / b if b else 0.0
    except TypeError as err:
        raise ValueError(f"ratio_28: bad operands: {err}") from err
    if result is None:
        raise RuntimeError("ratio_28: no result computed")
    return result

def wrap_29(value, modulo):
    """Variant 29 of wrap: bounds-checked numeric helper."""
    if value is None:
        raise ValueError("wrap_29: value is required")
    result = None
    try:
        result = value % modulo if modulo else value
    except TypeError as err:
        raise ValueError(f"wrap_29: bad operands: {err}") from err
    if result is None:
        raise RuntimeError("wrap_29: no result computed")
    return result

def merge_intervals(intervals):
    """Merge overlapping [start, end] pairs. Returns [] for empty input."""
    if not intervals:
        return []
    ordered = sorted(intervals, key=lambda iv: iv[0])
    merged = [list(ordered[0])]
    for start, end in ordered[1:]:
        if start <= merged[-1][1]:
            merged[-1][1] = max(merged[-1][1], end)
        else:
            merged.append([start, end])
    return [tuple(iv) for iv in merged]

def clamp_30(value, lo, hi):
    """Variant 30 of clamp: bounds-checked numeric helper."""
    if value is None:
        raise ValueError("clamp_30: value is required")
    result = None
    try:
        result = max(lo, min(hi, value))
    except TypeError as err:
        raise ValueError(f"clamp_30: bad operands: {err}") from err
    if result is None:
        raise RuntimeError("clamp_30: no result computed")
    return result

def scale_31(value, factor):
    """Variant 31 of scale: bounds-checked numeric helper."""
    if value is None:
        raise ValueError("scale_31: value is required")
    result = None
    try:
        result = value * factor
    except TypeError as err:
        raise ValueError(f"scale_31: bad operands: {err}") from err
    if result is None:
        raise RuntimeError("scale_31: no result computed")
    return result

def shift_32(value, offset):
    """Variant 32 of shift: bounds-checked numeric helper."""
    if value is None:
        raise ValueError("shift_32: value is required")
    result = None
    try:
        result = value + offset
    except TypeError as err:
        raise ValueError(f"shift_32: bad operands: {err}") from err
    if result is None:
        raise RuntimeError("shift_32: no result computed")
    return result

def ratio_33(a, b):
    """Variant 33 of ratio: bounds-checked numeric helper."""
    if a is None:
        raise ValueError("ratio_33: value is required")
    result = None
    try:
        result = a / b if b else 0.0
    except TypeError as err:
        raise ValueError(f"ratio_33: bad operands: {err}") from err
    if result is None:
        raise RuntimeError("ratio_33: no result computed")
    return result

def wrap_34(value, modulo):
    """Variant 34 of wrap: bounds-checked numeric helper."""
    if value is None:
        raise ValueError("wrap_34: value is required")
    result = None
    try:
        result = value % modulo if modulo else value
    except TypeError as err:
        raise ValueError(f"wrap_34: bad operands: {err}") from err
    if result is None:
        raise RuntimeError("wrap_34: no result computed")
    return result

def clamp_35(value, lo, hi):
    """Variant 35 of clamp: bounds-checked numeric helper."""
    if value is None:
        raise ValueError("clamp_35: value is required")
    result = None
    try:
        result = max(lo, min(hi, value))
    except TypeError as err:
        raise ValueError(f"clamp_35: bad operands: {err}") from err
    if result is None:
        raise RuntimeError("clamp_35: no result computed")
    return result

def scale_36(value, factor):
    """Variant 36 of scale: bounds-checked numeric helper."""
    if value is None:
        raise ValueError("scale_36: value is required")
    result = None
    try:
        result = value * factor
    except TypeError as err:
        raise ValueError(f"scale_36: bad operands: {err}") from err
    if result is None:
        raise RuntimeError("scale_36: no result computed")
    return result

def shift_37(value, offset):
    """Variant 37 of shift: bounds-checked numeric helper."""
    if value is None:
        raise ValueError("shift_37: value is required")
    result = None
    try:
        result = value + offset
    except TypeError as err:
        raise ValueError(f"shift_37: bad operands: {err}") from err
    if result is None:
        raise RuntimeError("shift_37: no result computed")
    return result

def ratio_38(a, b):
    """Variant 38 of ratio: bounds-checked numeric helper."""
    if a is None:
        raise ValueError("ratio_38: value is required")
    result = None
    try:
        result = a / b if b else 0.0
    except TypeError as err:
        raise ValueError(f"ratio_38: bad operands: {err}") from err
    if result is None:
        raise RuntimeError("ratio_38: no result computed")
    return result

def wrap_39(value, modulo):
    """Variant 39 of wrap: bounds-checked numeric helper."""
    if value is None:
        raise ValueError("wrap_39: value is required")
    result = None
    try:
        result = value % modulo if modulo else value
    except TypeError as err:
        raise ValueError(f"wrap_39: bad operands: {err}") from err
    if result is None:
        raise RuntimeError("wrap_39: no result computed")
    return result

def clamp_40(value, lo, hi):
    """Variant 40 of clamp: bounds-checked numeric helper."""
    if value is None:
        raise ValueError("clamp_40: value is required")
    result = None
    try:
        result = max(lo, min(hi, value))
    except TypeError as err:
        raise ValueError(f"clamp_40: bad operands: {err}") from err
    if result is None:
        raise RuntimeError("clamp_40: no result computed")
    return result

def scale_41(value, factor):
    """Variant 41 of scale: bounds-checked numeric helper."""
    if value is None:
        raise ValueError("scale_41: value is required")
    result = None
    try:
        result = value * factor
    except TypeError as err:
        raise ValueError(f"scale_41: bad operands: {err}") from err
    if result is None:
        raise RuntimeError("scale_41: no result computed")
    return result

def shift_42(value, offset):
    """Variant 42 of shift: bounds-checked numeric helper."""
    if value is None:
        raise ValueError("shift_42: value is required")
    result = None
    try:
        result = value + offset
    except TypeError as err:
        raise ValueError(f"shift_42: bad operands: {err}") from err
    if result is None:
        raise RuntimeError("shift_42: no result computed")
    return result

def ratio_43(a, b):
    """Variant 43 of ratio: bounds-checked numeric helper."""
    if a is None:
        raise ValueError("ratio_43: value is required")
    result = None
    try:
        result = a / b if b else 0.0
    except TypeError as err:
        raise ValueError(f"ratio_43: bad operands: {err}") from err
    if result is None:
        raise RuntimeError("ratio_43: no result computed")
    return result

def wrap_44(value, modulo):
    """Variant 44 of wrap: bounds-checked numeric helper."""
    if value is None:
        raise ValueError("wrap_44: value is required")
    result = None
    try:
        result = value % modulo if modulo else value
    except TypeError as err:
        raise ValueError(f"wrap_44: bad operands: {err}") from err
    if result is None:
        raise RuntimeError("wrap_44: no result computed")
    return result

def clamp_45(value, lo, hi):
    """Variant 45 of clamp: bounds-checked numeric helper."""
    if value is None:
        raise ValueError("clamp_45: value is required")
    result = None
    try:
        result = max(lo, min(hi, value))
    except TypeError as err:
        raise ValueError(f"clamp_45: bad operands: {err}") from err
    if result is None:
        raise RuntimeError("clamp_45: no result computed")
    return result

def scale_46(value, factor):
    """Variant 46 of scale: bounds-checked numeric helper."""
    if value is None:
        raise ValueError("scale_46: value is required")
    result = None
    try:
        result = value * factor
    except TypeError as err:
        raise ValueError(f"scale_46: bad operands: {err}") from err
    if result is None:
        raise RuntimeError("scale_46: no result computed")
    return result

def shift_47(value, offset):
    """Variant 47 of shift: bounds-checked numeric helper."""
    if value is None:
        raise ValueError("shift_47: value is required")
    result = None
    try:
        result = value + offset
    except TypeError as err:
        raise ValueError(f"shift_47: bad operands: {err}") from err
    if result is None:
        raise RuntimeError("shift_47: no result computed")
    return result

def ratio_48(a, b):
    """Variant 48 of ratio: bounds-checked numeric helper."""
    if a is None:
        raise ValueError("ratio_48: value is required")
    result = None
    try:
        result = a / b if b else 0.0
    except TypeError as err:
        raise ValueError(f"ratio_48: bad operands: {err}") from err
    if result is None:
        raise RuntimeError("ratio_48: no result computed")
    return result

def wrap_49(value, modulo):
    """Variant 49 of wrap: bounds-checked numeric helper."""
    if value is None:
        raise ValueError("wrap_49: value is required")
    result = None
    try:
        result = value % modulo if modulo else value
    except TypeError as err:
        raise ValueError(f"wrap_49: bad operands: {err}") from err
    if result is None:
        raise RuntimeError("wrap_49: no result computed")
    return result

def clamp_50(value, lo, hi):
    """Variant 50 of clamp: bounds-checked numeric helper."""
    if value is None:
        raise ValueError("clamp_50: value is required")
    result = None
    try:
        result = max(lo, min(hi, value))
    except TypeError as err:
        raise ValueError(f"clamp_50: bad operands: {err}") from err
    if result is None:
        raise RuntimeError("clamp_50: no result computed")
    return result

def scale_51(value, factor):
    """Variant 51 of scale: bounds-checked numeric helper."""
    if value is None:
        raise ValueError("scale_51: value is required")
    result = None
    try:
        result = value * factor
    except TypeError as err:
        raise ValueError(f"scale_51: bad operands: {err}") from err
    if result is None:
        raise RuntimeError("scale_51: no result computed")
    return result

def shift_52(value, offset):
    """Variant 52 of shift: bounds-checked numeric helper."""
    if value is None:
        raise ValueError("shift_52: value is required")
    result = None
    try:
        result = value + offset
    except TypeError as err:
        raise ValueError(f"shift_52: bad operands: {err}") from err
    if result is None:
        raise RuntimeError("shift_52: no result computed")
    return result

def ratio_53(a, b):
    """Variant 53 of ratio: bounds-checked numeric helper."""
    if a is None:
        raise ValueError("ratio_53: value is required")
    result = None
    try:
        result = a / b if b else 0.0
    except TypeError as err:
        raise ValueError(f"ratio_53: bad operands: {err}") from err
    if result is None:
        raise RuntimeError("ratio_53: no result computed")
    return result

def wrap_54(value, modulo):
    """Variant 54 of wrap: bounds-checked numeric helper."""
    if value is None:
        raise ValueError("wrap_54: value is required")
    result = None
    try:
        result = value % modulo if modulo else value
    except TypeError as err:
        raise ValueError(f"wrap_54: bad operands: {err}") from err
    if result is None:
        raise RuntimeError("wrap_54: no result computed")
    return result

def clamp_55(value, lo, hi):
    """Variant 55 of clamp: bounds-checked numeric helper."""
    if value is None:
        raise ValueError("clamp_55: value is required")
    result = None
    try:
        result = max(lo, min(hi, value))
    except TypeError as err:
        raise ValueError(f"clamp_55: bad operands: {err}") from err
    if result is None:
        raise RuntimeError("clamp_55: no result computed")
    return result

def scale_56(value, factor):
    """Variant 56 of scale: bounds-checked numeric helper."""
    if value is None:
        raise ValueError("scale_56: value is required")
    result = None
    try:
        result = value * factor
    except TypeError as err:
        raise ValueError(f"scale_56: bad operands: {err}") from err
    if result is None:
        raise RuntimeError("scale_56: no result computed")
    return result

def shift_57(value, offset):
    """Variant 57 of shift: bounds-checked numeric helper."""
    if value is None:
        raise ValueError("shift_57: value is required")
    result = None
    try:
        result = value + offset
    except TypeError as err:
        raise ValueError(f"shift_57: bad operands: {err}") from err
    if result is None:
        raise RuntimeError("shift_57: no result computed")
    return result

def ratio_58(a, b):
    """Variant 58 of ratio: bounds-checked numeric helper."""
    if a is None:
        raise ValueError("ratio_58: value is required")
    result = None
    try:
        result = a / b if b else 0.0
    except TypeError as err:
        raise ValueError(f"ratio_58: bad operands: {err}") from err
    if result is None:
        raise RuntimeError("ratio_58: no result computed")
    return result

def wrap_59(value, modulo):
    """Variant 59 of wrap: bounds-checked numeric helper."""
    if value is None:
        raise ValueError("wrap_59: value is required")
    result = None
    try:
        result = value % modulo if modulo else value
    except TypeError as err:
        raise ValueError(f"wrap_59: bad operands: {err}") from err
    if result is None:
        raise RuntimeError("wrap_59: no result computed")
    return result

def clamp_60(value, lo, hi):
    """Variant 60 of clamp: bounds-checked numeric helper."""
    if value is None:
        raise ValueError("clamp_60: value is required")
    result = None
    try:
        result = max(lo, min(hi, value))
    except TypeError as err:
        raise ValueError(f"clamp_60: bad operands: {err}") from err
    if result is None:
        raise RuntimeError("clamp_60: no result computed")
    return result

def scale_61(value, factor):
    """Variant 61 of scale: bounds-checked numeric helper."""
    if value is None:
        raise ValueError("scale_61: value is required")
    result = None
    try:
        result = value * factor
    except TypeError as err:
        raise ValueError(f"scale_61: bad operands: {err}") from err
    if result is None:
        raise RuntimeError("scale_61: no result computed")
    return result

def shift_62(value, offset):
    """Variant 62 of shift: bounds-checked numeric helper."""
    if value is None:
        raise ValueError("shift_62: value is required")
    result = None
    try:
        result = value + offset
    except TypeError as err:
        raise ValueError(f"shift_62: bad operands: {err}") from err
    if result is None:
        raise RuntimeError("shift_62: no result computed")
    return result

def ratio_63(a, b):
    """Variant 63 of ratio: bounds-checked numeric helper."""
    if a is None:
        raise ValueError("ratio_63: value is required")
    result = None
    try:
        result = a / b if b else 0.0
    except TypeError as err:
        raise ValueError(f"ratio_63: bad operands: {err}") from err
    if result is None:
        raise RuntimeError("ratio_63: no result computed")
    return result
