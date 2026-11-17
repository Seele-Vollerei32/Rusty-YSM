"""Validate SPM player display inputs before publishing them to the native bridge."""
import math
import re

RESOURCE = re.compile(r'[a-z0-9_.-]+:[a-z0-9_./-]+')
NUMBERS = {'health': (0, 1_000_000), 'max_health': (0, 1_000_000),
           'strafe_input': (-1, 1), 'vertical_input': (-1, 1), 'forward_input': (-1, 1)}
INTEGERS = {'experience_level': (0, 2**31 - 1), 'food_level': (0, 20)}
BOOLEANS = {'flying', 'shield_blocking'}


def validate_display(value):
    """Keep null/missing fields unknown; reject booleans masquerading as numbers."""
    if value is None:
        return None
    if not isinstance(value, dict) or set(value) != {'scope_id', 'world_epoch', 'dimension_id', 'state'}:
        raise ValueError('invalid display envelope')
    for key in ('scope_id', 'world_epoch'):
        text = value[key]
        if not isinstance(text, str) or not text or len(text) > 256:
            raise ValueError('invalid display context')
    dimension = value['dimension_id']
    if not isinstance(dimension, str) or len(dimension) > 256 or not RESOURCE.fullmatch(dimension):
        raise ValueError('invalid display dimension')
    state = value['state']
    if not isinstance(state, dict) or set(state) - (NUMBERS.keys() | INTEGERS.keys() | BOOLEANS | {'effect_amplifiers'}):
        raise ValueError('invalid display fields')
    for key, item in state.items():
        if item is None:
            continue
        if key in NUMBERS or key in INTEGERS:
            low, high = (NUMBERS | INTEGERS)[key]
            types = (int,) if key in INTEGERS else (int, float)
            if type(item) not in types or not low <= item <= high or not math.isfinite(item):
                raise ValueError('invalid display number')
        elif key in BOOLEANS:
            if type(item) is not bool:
                raise ValueError('invalid display boolean')
        else:
            if not isinstance(item, dict) or len(item) > 256:
                raise ValueError('invalid display effects')
            for name, amp in item.items():
                if (not isinstance(name, str) or len(name) > 256 or not RESOURCE.fullmatch(name)
                        or type(amp) is not int or not 1 <= amp <= 256):
                    raise ValueError('invalid display effect')
    return value
