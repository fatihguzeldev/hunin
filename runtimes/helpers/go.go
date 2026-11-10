package main

import (
	"bufio"
	"encoding/json"
	"fmt"
	"os"
	"runtime"
	"runtime/debug"
	"runtime/pprof"
	"strings"
	"time"
)

func writeJSON(path string, value any) {
	f, err := os.OpenFile(path, os.O_WRONLY|os.O_CREATE|os.O_EXCL, 0600)
	if err != nil {
		panic(err)
	}
	if err = json.NewEncoder(f).Encode(value); err != nil {
		panic(err)
	}
	if err = f.Sync(); err != nil {
		panic(err)
	}
	if err = f.Close(); err != nil {
		panic(err)
	}
}
func main() {
	profile, err := os.OpenFile(os.Args[1], os.O_WRONLY|os.O_CREATE|os.O_EXCL, 0600)
	if err != nil {
		panic(err)
	}
	if err = pprof.StartCPUProfile(profile); err != nil {
		panic(err)
	}
	input := bufio.NewReader(os.Stdin)
	fmt.Println("HUNIN_READY")
	command, _ := input.ReadString('\n')
	outcome := "cancelled"
	var output any
	var failure any
	origin := time.Now()
	var workStart, workEnd int64
	if strings.TrimSpace(command) == "HUNIN_GO" {
		outcome = "completed"
		workStart = time.Since(origin).Nanoseconds()
		func() {
			defer func() {
				if e := recover(); e != nil {
					outcome = "failed"
					failure = fmt.Sprint(e)
				}
			}()
			output = run()
		}()
		workEnd = time.Since(origin).Nanoseconds()
	}
	pprof.StopCPUProfile()
	if err = profile.Sync(); err != nil {
		panic(err)
	}
	if err = profile.Close(); err != nil {
		panic(err)
	}
	build, _ := debug.ReadBuildInfo()
	writeJSON(os.Args[2], map[string]any{"outcome": outcome, "output": output, "failure": failure, "runtime": runtime.Version(), "pid": os.Getpid(), "build": build, "clock": "time.Since helper origin monotonic nanoseconds; pprof profile uses its own epoch aggregate header", "work_start_ns": workStart, "work_end_ns": workEnd, "scope": "process-wide Go runtime CPU sampling; aggregate stacks, not per-call durations; c-archive/c-shared SIGPROF compatibility not claimed"})
	if os.Getenv("HUNIN_FINALIZE_GATE") == "1" {
		fmt.Println("HUNIN_WORK_COMPLETE")
		command, _ = input.ReadString('\n')
		if c := strings.TrimSpace(command); c != "HUNIN_EXIT" && c != "HUNIN_STOP" && c != "" {
			panic("invalid final command")
		}
	}
	if failure != nil {
		os.Exit(1)
	}
}
