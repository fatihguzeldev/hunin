<?php
declare(strict_types=1);

function resize(array $input): array {
    $pixels = [];
    for ($round = 0; $round < 1000; $round++) {
        foreach ($input as $index => $value) {
            $pixels[$index] = (($value * 3 + 7) ^ $value) & 255;
        }
    }
    return $pixels;
}

function run(): array {
    $input = range(0, 255);
    $pixels = [];
    for ($repetition = 0; $repetition < 7; $repetition++) {
        $pixels = resize($input);
    }
    return ['pixels' => $pixels, 'width' => 32, 'height' => 8, 'repetitions' => 7];
}
