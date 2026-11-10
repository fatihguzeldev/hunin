<?php
declare(strict_types=1);

[$script, $workload, $profile, $report] = $argv;
if (!extension_loaded('xdebug') || phpversion('xdebug') !== '3.5.3') {
    throw new RuntimeException('xdebug 3.5.3 must be explicitly loaded');
}
$actualProfile = xdebug_get_profiler_filename();
if (!is_string($actualProfile) || realpath($actualProfile) !== realpath($profile)) {
    throw new RuntimeException('active xdebug output differs from the prepared project path');
}
$outcome = 'stopped_before_work';
$output = null;
$applicationError = null;
$workStart = null;
$workEnd = null;
echo "HUNIN_READY\n";
flush();
$command = rtrim((string) fgets(STDIN), "\r\n");
try {
    if ($command === 'HUNIN_GO') {
        $workStart = hrtime(true);
        require $workload;
        if (!function_exists('run')) {
            throw new RuntimeException('prepared php fixture must expose run()');
        }
        $output = run();
        $outcome = 'completed';
    } elseif ($command !== 'HUNIN_STOP' && $command !== '') {
        throw new RuntimeException('invalid prepared-work gate command');
    }
} catch (Throwable $error) {
    $outcome = 'application_failed';
    $applicationError = (string) $error;
} finally {
    if ($workStart !== null) {
        $workEnd = hrtime(true);
    }
}
$stream = fopen($report, 'x');
if ($stream === false) {
    throw new RuntimeException('cannot create prepared report');
}
$encoded = json_encode([
    'outcome' => $outcome, 'output' => $output, 'application_error' => $applicationError,
    'runtime' => ['php' => PHP_VERSION, 'xdebug' => phpversion('xdebug'), 'pid' => getmypid()],
    'profile_path' => $actualProfile, 'helper_clock' => 'hrtime(true)',
    'work_start_ns' => $workStart, 'work_end_ns' => $workEnd,
    'coverage' => 'startup through process shutdown; profile finalizes only at owned exit',
], JSON_THROW_ON_ERROR);
if (fwrite($stream, $encoded) !== strlen($encoded) || !fflush($stream) || !fsync($stream)) {
    throw new RuntimeException('cannot persist prepared report');
}
fclose($stream);
if (getenv('HUNIN_FINALIZE_GATE') === '1') {
    echo "HUNIN_WORK_COMPLETE\n";
    flush();
    $finish = rtrim((string) fgets(STDIN), "\r\n");
    if (!in_array($finish, ['HUNIN_EXIT', 'HUNIN_STOP', ''], true)) {
        throw new RuntimeException('invalid finalize gate command');
    }
}
exit($applicationError === null ? 0 : 1);
