"""Host checks for strict gain evidence interpretation; no vendor inputs."""
import copy
import unittest
from phy_gain import calls, output, publication


class GainEvidenceTests(unittest.TestCase):
    def test_output_rejects_unknown_unavailable_and_noncontiguous_bytes(self):
        record = dict(kind='final-memory', case=1, replacement=False,
                      chunk=dict(selection=0, offset=0, length=2,
                                 bytes=[0x12, 0x34], known=3, available=3))
        self.assertEqual(output([record], 1), b'\x12\x34')
        for field, value in [('known', 1), ('available', 1), ('offset', 1), ('selection', 1)]:
            changed = copy.deepcopy(record)
            changed['chunk'][field] = value
            with self.assertRaises(AssertionError):
                output([changed], 1)
        with self.assertRaises(AssertionError):
            output([record, record], 1)
        with self.assertRaises(AssertionError):
            output([record], 2)

    def test_kernel_boundary_rejects_model_and_unknown_argument(self):
        call = dict(kind='call-transfer', target=10, target_kind='captured-code', words=1)
        arg = dict(kind='transfer-argument', value=dict(kind='known', value=123))
        self.assertEqual(calls([call, arg], 10), [[123]])
        for changed in [dict(call, target_kind='call-model')]:
            with self.assertRaises(AssertionError):
                calls([changed, arg], 10)
        with self.assertRaises(AssertionError):
            calls([call, dict(arg, value=dict(kind='unknown'))], 10)
        with self.assertRaises(AssertionError):
            calls([call], 10)

    def test_publisher_oracle_retains_index_control_and_wraps_bank(self):
        wifi = publication([0]*6, 0, bytes(160), 32, 255, 0xa5)
        self.assertEqual(len(wifi), 161)
        self.assertEqual(wifi[1:4], [('write', 0x20100848, 0),
                                    ('write', 0x2010084c, 0x10000000),
                                    ('write', 0x20100850, 0x7f80)])
        self.assertEqual(wifi[4], ('read', 0x20100844, 0xa5a5a5a5))
        self.assertEqual(wifi[5], ('write', 0x20100844, 0xa5aff800))
        self.assertEqual(wifi[10], ('write', 0x20100844, 0xa5a80000))
        bluetooth = publication([0]*6, 0, bytes(80), 16, 224, 0)
        # Default is Wi-Fi bank; explicit BT selects +32 and wraps to zero.
        self.assertEqual(bluetooth[5][2], 0xf0000)
        bluetooth = publication([0]*6, 0, bytes(80), 16, 224, 0, True)
        self.assertEqual(len(bluetooth), 81)
        self.assertEqual(bluetooth[5][2], 0x80000)


class GainStateTests(unittest.TestCase):
    def test_mac_power_updates_both_index_fields_and_preserves_other_bits(self):
        from phy_gain_state import mac_power
        self.assertEqual(mac_power(21, 0xa5a5a5a5), [
            ('read', 0x20105500, 0xa5a5a5a5), ('write', 0x20105500, 0xa5a5a595),
            ('read', 0x20105500, 0xa5a5a595), ('write', 0x20105500, 0xa5a59595)])
        # Signed policy results are truncated to the six-bit fields.
        self.assertEqual(mac_power(-12, 0)[3], ('write', 0x20105500, 0x3434))

    def test_missing_rftest_is_an_unmet_obligation(self):
        from phy_gain_state import exercise, RFTEST_OBLIGATION
        import phy_gain_state
        ran = []
        original = phy_gain_state.storage, phy_gain_state.storage_negative, phy_gain_state.producer
        try:
            phy_gain_state.storage = lambda g: ran.append('storage') or 'baseline'
            phy_gain_state.storage_negative = lambda g, b: ran.append(('storage-negative', b))
            phy_gain_state.producer = lambda g: self.fail('producer requires RF-test input')
            self.assertEqual(exercise(object(), False), [RFTEST_OBLIGATION])
        finally:
            phy_gain_state.storage, phy_gain_state.storage_negative, phy_gain_state.producer = original
        self.assertEqual(ran, ['storage', ('storage-negative', 'baseline')])


if __name__ == '__main__':
    unittest.main()
