package main

//go:noinline
func resize(input, output []int) {
	for i, value := range input {
		output[i] = ((value*3 + 7) ^ value) & 255
	}
}
func run() any {
	input := make([]int, 256*256)
	for i := range input {
		input[i] = i & 255
	}
	output := make([]int, len(input))
	for batch := 0; batch < 4096; batch++ {
		resize(input, output)
	}
	return map[string]any{"pixels": output, "width": 256, "height": 256, "batches": 4096}
}
