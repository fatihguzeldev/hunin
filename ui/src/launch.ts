import type { Capability } from './api';

export type LaunchChoice = {
  value: string;
  label: string;
  launchRuntime?: string;
  commandRequirement?: string;
  defaultExecutable?: string | null;
  availability: string;
  recordingVerified: boolean;
  unavailableReason?: string;
};

/** The service owns the launch route; discovery probes are not launch guarantees. */
export function launchChoices(runtimes: Capability[]): LaunchChoice[] {
  return runtimes.flatMap<LaunchChoice>((runtime) => {
    if (!runtime.modes?.length)
      return [
        {
          value: JSON.stringify([runtime.runtime]),
          label: `${runtime.runtime} · launch metadata unavailable`,
          availability: runtime.probe_status,
          recordingVerified: false,
          unavailableReason:
            'This service did not provide collection modes for this runtime.',
        },
      ];
    return runtime.modes.map((mode) => ({
      value: JSON.stringify([runtime.runtime, mode.mode]),
      label: `${runtime.runtime} · ${mode.mode}`,
      launchRuntime:
        mode.selectable === true && mode.launch_runtime
          ? mode.launch_runtime
          : undefined,
      commandRequirement: mode.command_requirement,
      defaultExecutable: mode.default_executable,
      availability: mode.availability,
      recordingVerified: mode.recording_verified,
      unavailableReason:
        mode.selectable === false
          ? 'This collection mode cannot be launched by this service.'
          : mode.selectable !== true || !mode.launch_runtime
            ? 'This service did not provide a selectable launch route for this mode.'
            : undefined,
    }));
  });
}
