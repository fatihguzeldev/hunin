require 'json'
require 'fiber'

workload, profile_path, report_path = ARGV
unless [workload, profile_path, report_path].all? { |p| p && File.absolute_path?(p) }
  raise 'helper paths must be absolute'
end

events = []
threads = {}
fibers = {}
lost = 0
bytes = 0
helper = File.expand_path(__FILE__)
selected = [:call, :return, :raise, :thread_begin, :thread_end, :fiber_switch]
trace = TracePoint.new(*selected) do |point|
  next if File.expand_path(point.path || '') == helper
  if events.length >= 100_000 || bytes >= 8 * 1024 * 1024
    lost += 1
    next
  end
  thread = Thread.current
  fiber = Fiber.current
  threads[thread] ||= threads.length
  fibers[fiber] ||= fibers.length
  event = {
    'event' => point.event.to_s,
    'timestamp_ns' => Process.clock_gettime(Process::CLOCK_MONOTONIC, :nanosecond),
    'thread' => threads[thread], 'fiber' => fibers[fiber],
    'file' => point.path, 'line' => point.lineno,
    'method' => point.method_id && point.method_id.to_s,
  }
  bytes += JSON.generate(event).bytesize
  events << event
end

output = nil
application_error = nil
outcome = 'stopped_before_work'
work_start_ns = nil
work_end_ns = nil
trace.enable
STDOUT.write("HUNIN_READY\n")
STDOUT.flush
command = STDIN.gets
begin
  if command == "HUNIN_GO\n"
    work_start_ns = Process.clock_gettime(Process::CLOCK_MONOTONIC, :nanosecond)
    load workload
    output = HuninWorkload.run
    outcome = 'completed'
  elsif command && command != "HUNIN_STOP\n"
    raise 'invalid prepared-work gate command'
  end
rescue Exception => error
  application_error = "#{error.class}: #{error.message}"
  outcome = 'application_failed'
ensure
  work_end_ns = Process.clock_gettime(Process::CLOCK_MONOTONIC, :nanosecond) if work_start_ns
  trace.disable
end

File.open(profile_path, 'wx') do |stream|
  stream.write(JSON.generate({
    'runtime' => RUBY_ENGINE, 'version' => RUBY_VERSION, 'pid' => Process.pid,
    'clock' => 'Process::CLOCK_MONOTONIC nanoseconds',
    'event_set' => selected.map(&:to_s), 'events' => events, 'lost_events' => lost,
    'coverage' => 'ruby method/exception/thread/fiber events; blocks and native C calls excluded',
  }))
  stream.flush
  stream.fsync
end
File.open(report_path, 'wx') do |stream|
  stream.write(JSON.generate({'outcome' => outcome, 'output' => output, 'application_error' => application_error,
    'work_start_ns' => work_start_ns, 'work_end_ns' => work_end_ns, 'helper_clock' => 'Process::CLOCK_MONOTONIC nanoseconds'}))
  stream.flush
  stream.fsync
end
if ENV['HUNIN_FINALIZE_GATE'] == '1'
  STDOUT.write("HUNIN_WORK_COMPLETE\n")
  STDOUT.flush
  finish = STDIN.gets
  raise 'invalid finalize gate command' if finish && !["HUNIN_EXIT\n", "HUNIN_STOP\n"].include?(finish)
end
exit(application_error ? 1 : 0)
