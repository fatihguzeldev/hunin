<?php
declare(strict_types=1);

function ppm_space(string $value): bool {
    return str_contains("\t\n\v\f\r ", $value);
}

function ppm_integer(string $value): int {
    if ($value === '' || strspn($value, '0123456789') !== strlen($value)) throw new RuntimeException('expected unsigned decimal integer');
    return (int) $value;
}

function ppm_token(string $bytes, int &$position): string {
    $length = strlen($bytes);
    while ($position < $length) {
        if ($bytes[$position] === '#') {
            while ($position < $length && $bytes[$position] !== "\n") $position++;
        } elseif (ppm_space($bytes[$position])) {
            $position++;
        } else {
            break;
        }
    }
    $start = $position;
    while ($position < $length && !ppm_space($bytes[$position])) $position++;
    if ($start === $position) throw new RuntimeException('missing ppm header token');
    return substr($bytes, $start, $position - $start);
}

function resize(string $source, int $width, int $height): string {
    $output = '';
    for ($y = 0; $y < 32; $y++) {
        $sourceY = intdiv($y * $height, 32);
        for ($x = 0; $x < 32; $x++) {
            $position = ($sourceY * $width + intdiv($x * $width, 32)) * 3;
            $output .= substr($source, $position, 3);
        }
    }
    return "P6\n32 32\n255\n" . $output;
}

function run(): array {
    $input = getenv('PREVIEW_INPUT');
    $output = getenv('PREVIEW_OUTPUT');
    $configured = getenv('PREVIEW_REPETITIONS');
    $repetitions = $configured === false ? 1 : ppm_integer($configured);
    if (!is_string($input) || !is_string($output) || $repetitions < 1) {
        throw new RuntimeException('preview input, output and positive repetitions are required');
    }
    if (!is_dir($output) && !mkdir($output, 0700, true)) throw new RuntimeException('cannot create preview output');
    $names = array_values(array_filter(scandir($input), static fn(string $name): bool => str_ends_with($name, '.ppm') && is_file($input . DIRECTORY_SEPARATOR . $name)));
    sort($names, SORT_STRING);
    $assets = [];
    foreach ($names as $name) {
        $source = file_get_contents($input . DIRECTORY_SEPARATOR . $name);
        if ($source === false) throw new RuntimeException('cannot read preview input');
        $position = 0;
        $magic = ppm_token($source, $position);
        $widthToken = ppm_token($source, $position);
        $heightToken = ppm_token($source, $position);
        $maximum = ppm_token($source, $position);
        $width = ppm_integer($widthToken);
        $height = ppm_integer($heightToken);
        if ($magic !== 'P6' || $maximum !== '255' || $width < 1 || $height < 1 || $width > 65536 || $height > 65536) {
            throw new RuntimeException('unsupported ppm input');
        }
        if (!isset($source[$position]) || !ppm_space($source[$position])) throw new RuntimeException('missing ppm payload separator');
        $position += substr($source, $position, 2) === "\r\n" ? 2 : 1;
        $pixels = substr($source, $position);
        if (strlen($pixels) !== $width * $height * 3) throw new RuntimeException('ppm payload length differs from dimensions');
        $preview = '';
        for ($repetition = 0; $repetition < $repetitions; $repetition++) $preview = resize($pixels, $width, $height);
        $destination = substr($name, 0, -4) . '.preview.ppm';
        if (file_put_contents($output . DIRECTORY_SEPARATOR . $destination, $preview) !== strlen($preview)) throw new RuntimeException('cannot write preview');
        $assets[] = ['input' => $name, 'output' => $destination, 'source_width' => $width, 'source_height' => $height,
            'bytes' => strlen($preview), 'sha256' => hash('sha256', $preview)];
    }
    $manifest = ['algorithm' => 'nearest-neighbor-rgb8', 'width' => 32, 'height' => 32, 'assets' => $assets];
    if (file_put_contents($output . DIRECTORY_SEPARATOR . 'manifest.json', json_encode($manifest, JSON_THROW_ON_ERROR)) === false) throw new RuntimeException('cannot write preview manifest');
    return $manifest;
}
