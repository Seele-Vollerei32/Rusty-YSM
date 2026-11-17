import subprocess
import unittest
from unittest.mock import Mock, patch
from cloud_play import main, stop_process


class CloudLaunchTests(unittest.TestCase):
    def test_missing_client_starts_no_process(self):
        with patch('sys.argv', ['cloud_play.py', '--origin', 'https://cloud.example',
                                '--resources', '.', '--client', '__missing_client__']), \
                patch('cloud_play.subprocess.Popen') as popen, \
                patch('sys.stderr'), self.assertRaises(SystemExit) as error:
            main()
        self.assertEqual(error.exception.code, 2)
        popen.assert_not_called()

    def test_child_termination_escalates_and_reaps(self):
        process = Mock()
        process.poll.return_value = None
        process.wait.side_effect = [subprocess.TimeoutExpired('child', 5), 0]
        stop_process(process)
        process.terminate.assert_called_once()
        process.kill.assert_called_once()
        self.assertEqual(process.wait.call_count, 2)

    def test_finished_child_is_not_signalled(self):
        process = Mock()
        process.poll.return_value = 0
        stop_process(process)
        process.terminate.assert_not_called()
        process.kill.assert_not_called()


if __name__ == '__main__':
    unittest.main()
