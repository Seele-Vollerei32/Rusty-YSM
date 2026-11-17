import unittest
from display_state import validate_display


class DisplayTests(unittest.TestCase):
    def envelope(self, state):
        return {'scope_id': 'test', 'world_epoch': 'epoch',
                'dimension_id': 'minecraft:overworld', 'state': state}

    def test_unknowns_and_valid_boundaries(self):
        self.assertIsNone(validate_display(None))
        state = {'health': None, 'max_health': 1_000_000, 'experience_level': 2**31 - 1,
                 'food_level': 20, 'strafe_input': -1, 'forward_input': 1,
                 'flying': False, 'shield_blocking': True,
                 'effect_amplifiers': {'minecraft:speed': 256}}
        self.assertEqual(validate_display(self.envelope(state))['state'], state)

    def test_invalid_field_values(self):
        for key, value in [('health', True), ('health', float('nan')),
                           ('health', float('inf')), ('health', -1),
                           ('max_health', 1_000_001), ('food_level', 20.0),
                           ('experience_level', 2**31), ('flying', 1),
                           ('forward_input', 1.01), ('unknown', 0),
                           ('effect_amplifiers', {'minecraft:speed': 0}),
                           ('effect_amplifiers', {'bad': 1}),
                           ('effect_amplifiers', {'minecraft:speed': True})]:
            with self.subTest(key=key, value=value), self.assertRaises(ValueError):
                validate_display(self.envelope({key: value}))

    def test_invalid_envelopes(self):
        for value in [[], {}, self.envelope([]),
                      dict(self.envelope({}), dimension_id='BAD:dimension'),
                      dict(self.envelope({}), world_epoch='')]:
            with self.subTest(value=value), self.assertRaises(ValueError):
                validate_display(value)


if __name__ == '__main__':
    unittest.main()
