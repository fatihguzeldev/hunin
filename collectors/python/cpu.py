"""Stable entrypoint for the Python capture component."""

from hunin_capture.cpu_progress.sampling import (
    Sampler as Sampler,
    main as main,
)


if __name__ == "__main__":
    main()
