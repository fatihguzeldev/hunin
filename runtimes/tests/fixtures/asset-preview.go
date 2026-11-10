package main

import (
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"sort"
	"strconv"
	"strings"
)

type previewImage struct {
	width, height int
	pixels        []byte
}

func readImage(data []byte) previewImage {
	cursor := 0
	space := func(c byte) bool { return c == 9 || c == 10 || c == 13 || c == 32 }
	token := func() string {
		for cursor < len(data) {
			if data[cursor] == '#' {
				for cursor < len(data) && data[cursor] != 10 {
					cursor++
				}
			} else if space(data[cursor]) {
				cursor++
			} else {
				break
			}
		}
		start := cursor
		for cursor < len(data) && !space(data[cursor]) && data[cursor] != '#' {
			cursor++
		}
		return string(data[start:cursor])
	}
	if token() != "P6" {
		panic("expected P6 RGB image")
	}
	width, e1 := strconv.Atoi(token())
	height, e2 := strconv.Atoi(token())
	maximum, e3 := strconv.Atoi(token())
	if e1 != nil || e2 != nil || e3 != nil || width < 1 || width > 4096 || height < 1 || height > 4096 || maximum != 255 {
		panic("invalid RGB8 dimensions")
	}
	if cursor+1 < len(data) && data[cursor] == 13 && data[cursor+1] == 10 {
		cursor += 2
	} else if cursor < len(data) && space(data[cursor]) {
		cursor++
	} else {
		panic("missing PPM pixel delimiter")
	}
	pixels := data[cursor:]
	if len(pixels) != width*height*3 {
		panic("unexpected PPM pixel length")
	}
	return previewImage{width, height, pixels}
}

//go:noinline
func resize(image previewImage, output []byte) []byte {
	for y := 0; y < 32; y++ {
		for x := 0; x < 32; x++ {
			from := ((y*image.height/32)*image.width + x*image.width/32) * 3
			to := (y*32 + x) * 3
			copy(output[to:to+3], image.pixels[from:from+3])
		}
	}
	return output
}
func run() any {
	input, output := os.Getenv("PREVIEW_INPUT"), os.Getenv("PREVIEW_OUTPUT")
	text := os.Getenv("PREVIEW_REPETITIONS")
	if text == "" {
		text = "50000"
	}
	repetitions, err := strconv.Atoi(text)
	if err != nil || !filepath.IsAbs(input) || !filepath.IsAbs(output) || repetitions < 1 || repetitions > 100000 {
		panic("invalid preview input/output/repetitions")
	}
	if err := os.MkdirAll(output, 0700); err != nil {
		panic(err)
	}
	entries, err := os.ReadDir(input)
	if err != nil {
		panic(err)
	}
	names := []string{}
	for _, entry := range entries {
		if !entry.IsDir() && strings.HasSuffix(entry.Name(), ".ppm") {
			names = append(names, entry.Name())
		}
	}
	sort.Strings(names)
	assets := []any{}
	for _, name := range names {
		data, err := os.ReadFile(filepath.Join(input, name))
		if err != nil {
			panic(err)
		}
		image := readImage(data)
		pixels := make([]byte, 32*32*3)
		for pass := 0; pass < repetitions; pass++ {
			pixels = resize(image, pixels)
		}
		result := append([]byte("P6\n32 32\n255\n"), pixels...)
		filename := strings.TrimSuffix(name, ".ppm") + ".preview.ppm"
		if err := os.WriteFile(filepath.Join(output, filename), result, 0600); err != nil {
			panic(err)
		}
		digest := sha256.Sum256(result)
		assets = append(assets, map[string]any{"input": name, "output": filename, "source_width": image.width, "source_height": image.height, "bytes": len(result), "sha256": hex.EncodeToString(digest[:])})
	}
	if len(assets) == 0 {
		panic("preview directory has no PPM assets")
	}
	manifest := map[string]any{"algorithm": "nearest-neighbor-rgb8", "width": 32, "height": 32, "assets": assets}
	encoded, err := json.Marshal(manifest)
	if err != nil {
		panic(err)
	}
	if err := os.WriteFile(filepath.Join(output, "manifest.json"), encoded, 0600); err != nil {
		panic(fmt.Sprint(err))
	}
	return manifest
}
