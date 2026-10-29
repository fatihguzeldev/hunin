package control

import "testing"

var Result uint64

//go:noinline
func huninGoHotspot(rounds uint64) uint64 {
	value := uint64(1)
	for item := uint64(0); item < rounds; item++ {
		value = (value^item)*6364136223846793005 + 1
	}
	return value
}

func TestCPU(t *testing.T) {
	Result = huninGoHotspot(250000000)
	if Result != 12795955527751812353 {
		t.Fatalf("incorrect control result: %d", Result)
	}
}
