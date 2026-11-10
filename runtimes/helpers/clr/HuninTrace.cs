using Microsoft.Diagnostics.NETCore.Client;
using Microsoft.Diagnostics.Tracing;
using Microsoft.Diagnostics.Tracing.Etlx;
using System.Diagnostics;
using System.Diagnostics.Tracing;
using System.Reflection;
using System.Reflection.Metadata;
using System.Reflection.Metadata.Ecma335;
using System.Reflection.PortableExecutable;
using System.Security.Cryptography;
using System.Text;
using System.Text.Json;

internal static class HuninTrace
{
    const long MaxTraceBytes = 16 * 1024 * 1024;
    const int MaxRecords = 100000;
    const int MaxJsonBytes = 16 * 1024 * 1024;
    static readonly List<string> Warnings = [];

    static async Task<int> Main(string[] args)
    {
        if (args.Length != 3) throw new ArgumentException("expected original workload, profile and report paths");
        var workload = Path.GetFullPath(args[0]);
        var profile = Path.GetFullPath(args[1]);
        var report = Path.GetFullPath(args[2]);
        var area = Path.GetDirectoryName(profile)!;
        if (!OperatingSystem.IsWindows())
        {
            var endpoint = Path.Combine(Path.GetTempPath(), $"dotnet-diagnostic-{Environment.ProcessId}-{DateTimeOffset.UtcNow.ToUnixTimeSeconds()}-socket");
            if (Encoding.UTF8.GetByteCount(endpoint) >= 104)
                throw new InvalidOperationException("project temporary path is too long for the diagnostic socket; configure a shorter project data root");
        }
        var raw = Path.Combine(area, "runtime.nettrace");
        var providers = new[] {
            new EventPipeProvider("Microsoft-DotNETCore-SampleProfiler", EventLevel.Informational, 0xF00000000000),
            new EventPipeProvider("Microsoft-Windows-DotNETRuntime", EventLevel.Informational, 0x100003801D),
        };
        object? output = null;
        string? applicationError = null;
        string outcome = "stopped_before_work";
        long? workStart = null, workEnd = null;
        using (var session = new DiagnosticsClient(Environment.ProcessId).StartEventPipeSession(providers, true, 16))
        {
            using var file = new FileStream(raw, FileMode.CreateNew, FileAccess.Write);
            var copying = CopyBounded(session.EventStream, file);
            Console.WriteLine("HUNIN_READY");
            Console.Out.Flush();
            var command = Console.ReadLine();
            try
            {
                if (command == "HUNIN_GO")
                {
                    workStart = Stopwatch.GetTimestamp();
                    var run = Assembly.GetExecutingAssembly().GetTypes()
                        .Select(type => type.GetMethod("Run", BindingFlags.Static | BindingFlags.Public, null, Type.EmptyTypes, null))
                        .Where(method => method != null).SingleOrDefault()
                        ?? throw new InvalidOperationException("fixture must expose exactly one public static parameterless Run()");
                    output = run.Invoke(null, null);
                    outcome = "completed";
                }
                else if (command is not ("HUNIN_STOP" or null or ""))
                    throw new InvalidOperationException("invalid prepared-work gate command");
            }
            catch (Exception error)
            {
                outcome = "application_failed";
                applicationError = error.ToString();
            }
            finally
            {
                if (workStart != null) workEnd = Stopwatch.GetTimestamp();
                session.Stop();
                await copying;
                file.Flush(true);
            }
        }
        Export(raw, profile, workload);
        WriteNew(report, new { outcome, output, application_error = applicationError,
            runtime = new { clr = Environment.Version.ToString(), pid = Environment.ProcessId },
            work_start_qpc = workStart, work_end_qpc = workEnd, helper_frequency = Stopwatch.Frequency,
            coverage = "acknowledged in-process EventPipe session before gate; runtime startup excluded" });
        if (Environment.GetEnvironmentVariable("HUNIN_FINALIZE_GATE") == "1")
        {
            Console.WriteLine("HUNIN_WORK_COMPLETE");Console.Out.Flush();
            if (Console.ReadLine() is not ("HUNIN_EXIT" or "HUNIN_STOP" or null or ""))
                throw new InvalidOperationException("invalid finalization gate command");
        }
        return applicationError == null ? 0 : 1;
    }

    static async Task CopyBounded(Stream source, Stream target)
    {
        var buffer = new byte[65536];long total = 0;
        int count;
        while ((count = await source.ReadAsync(buffer)) != 0)
        {
            total += count;
            if (total > MaxTraceBytes) throw new InvalidOperationException("EventPipe input exceeds 16 mib capture limit");
            await target.WriteAsync(buffer.AsMemory(0, count));
        }
    }

    static long ClockField(TraceEventSource source, string name)
    {
        // TraceEvent 4.0.0 exposes raw timestamps publicly, but keeps its decoded
        // source frequency/anchor internal. Fail if the pinned layout changes.
        var field = typeof(TraceEventSource).GetField(name, BindingFlags.Instance | BindingFlags.NonPublic)
            ?? throw new InvalidOperationException($"pinned TraceEvent clock field is unavailable: {name}");
        return (long)(field.GetValue(source) ?? throw new InvalidOperationException("missing trace clock"));
    }

    static void Export(string raw, string profile, string workload)
    {
        var etlx = TraceLog.CreateFromEventPipeDataFile(raw, Path.ChangeExtension(raw, ".etlx"));
        using var log = new TraceLog(etlx);
        var frequency = ClockField(log, "_QPCFreq");
        var origin = ClockField(log, "_syncTimeQPC");
        if (frequency <= 0) throw new InvalidOperationException("invalid EventPipe frequency");
        var uncertainty = Math.Max(1UL, (ulong)((1000000000L + (Int128)frequency - 1) / frequency));
        using var symbols = new PortableSymbols(Assembly.GetExecutingAssembly().Location, workload);
        var frames = new List<object>();var stacks = new List<object>();var events = new List<object>();
        var seenFrames = new HashSet<int>();var seenStacks = new HashSet<int>();
        int bytes = 0;
        void Add(List<object> target, object value)
        {
            if (frames.Count + stacks.Count + events.Count >= MaxRecords) throw new InvalidOperationException("EventPipe extracted record limit exceeded");
            bytes = checked(bytes + JsonSerializer.SerializeToUtf8Bytes(value).Length);
            if (bytes > MaxJsonBytes) throw new InvalidOperationException("EventPipe extracted byte limit exceeded");
            target.Add(value);
        }
        foreach (var data in log.Events)
        {
            if (data.ProcessID != Environment.ProcessId) continue;
            var stack = data.CallStack();
            int? head = stack == null ? null : (int)stack.CallStackIndex;
            int depth = 0;
            for (var current = stack; current != null; current = current.Caller)
            {
                if (++depth > 1024) throw new InvalidOperationException("EventPipe stack depth exceeds 1024");
                var stackId = (int)current.CallStackIndex;
                if (!seenStacks.Add(stackId)) break;
                var address = current.CodeAddress;var frameId = (int)address.CodeAddressIndex;
                Add(stacks, new { id = stackId, frame = frameId, caller = current.Caller == null ? (int?)null : (int)current.Caller.CallStackIndex });
                if (seenFrames.Add(frameId))
                {
                    var token = address.Method?.MethodToken;
                    var module = address.ModuleFile;
                    var location = symbols.Resolve(module?.FilePath, token, address.ILOffset, module?.PdbSignature);
                    Add(frames, new { id = frameId, address = address.Address, function = EmptyNull(address.FullMethodName),
                        module = module?.FilePath, module_pdb = module?.PdbSignature.ToString(), token,
                        il_offset = address.ILOffset, source_file = location.File, source_line = location.Line,
                        source_mapping = location.Meaning });
                }
            }
            var payload = new Dictionary<string, JsonElement>();
            foreach (var name in data.PayloadNames)
            {
                var value = data.PayloadByName(name);
                payload.Add(name, JsonSerializer.SerializeToElement(value, value?.GetType() ?? typeof(object)));
            }
            var relative = checked((long)(((Int128)data.TimeStampQPC - origin) * 1000000000L / frequency));
            Add(events, new { id = (int)data.EventIndex, provider = data.ProviderName, provider_guid = data.ProviderGuid,
                name = data.EventName, event_id = (int)data.ID, version = data.Version,
                process = data.ProcessID, thread = data.ThreadID, time_qpc = data.TimeStampQPC,
                time_ns = relative, uncertainty_ns = uncertainty, activity = data.ActivityID, related_activity = data.RelatedActivityID,
                stack = head, payload });
        }
        WriteNew(profile, new { format = "hunin-eventpipe-v1", traceevent = "4.0.0", runtime = Environment.Version.ToString(),
            frequency, origin_qpc = origin, events_lost = log.EventsLost,
            clock_projection = "signed integer qpc difference scaled to nanoseconds; raw source ticks retained", 
            providers = new[] { "Microsoft-DotNETCore-SampleProfiler:0xF00000000000:4", "Microsoft-Windows-DotNETRuntime:0x100003801D:4" },
            source_binding = symbols.Binding, warnings = Warnings, frames, stacks, events });
    }

    static string? EmptyNull(string? value) => string.IsNullOrEmpty(value) ? null : value;

    static void WriteNew(string path, object value)
    {
        using var file = new FileStream(path, FileMode.CreateNew, FileAccess.Write);
        using var bounded = new LimitedWriteStream(file, MaxJsonBytes);
        JsonSerializer.Serialize(bounded, value);bounded.Flush();file.Flush(true);
    }

    sealed class LimitedWriteStream(Stream stream, long maximum) : Stream
    {
        long Written;
        public override bool CanRead => false;
        public override bool CanSeek => false;
        public override bool CanWrite => true;
        public override long Length => Written;
        public override long Position { get => Written; set => throw new NotSupportedException(); }
        public override void Flush() => stream.Flush();
        public override void Write(byte[] bytes, int offset, int count) => Write(bytes.AsSpan(offset, count));
        public override void Write(ReadOnlySpan<byte> bytes)
        {
            if (bytes.Length > maximum - Written) throw new InvalidOperationException("structured EventPipe output exceeds limit");
            stream.Write(bytes);Written += bytes.Length;
        }
        public override int Read(byte[] bytes, int offset, int count) => throw new NotSupportedException();
        public override long Seek(long offset, SeekOrigin origin) => throw new NotSupportedException();
        public override void SetLength(long length) => throw new NotSupportedException();
    }

    sealed class PortableSymbols : IDisposable
    {
        readonly string AssemblyPath;
        readonly string Workload;
        MetadataReaderProvider? Provider;
        MetadataReader? Reader;
        Guid PdbGuid;
        byte[]? SourceSha256;
        byte[]? SourceSha1;
        public object Binding { get; }

        public PortableSymbols(string assembly, string workload)
        {
            AssemblyPath = Path.GetFullPath(assembly);Workload = workload;
            try
            {
                using var stream = File.OpenRead(assembly);using var pe = new PEReader(stream);
                var metadata = pe.GetMetadataReader();var mvid = metadata.GetGuid(metadata.GetModuleDefinition().Mvid);
                var debug = pe.ReadDebugDirectory().Single(entry => entry.Type == DebugDirectoryEntryType.CodeView);
                var codeview = pe.ReadCodeViewDebugDirectoryData(debug);
                var pdb = File.OpenRead(Path.ChangeExtension(assembly, ".pdb"));
                if (pdb.Length > MaxTraceBytes) { pdb.Dispose();throw new InvalidOperationException("portable pdb exceeds 16 mib source-metadata limit"); }
                Provider = MetadataReaderProvider.FromPortablePdbStream(pdb);
                var reader = Provider.GetMetadataReader();var content = new BlobContentId(reader.DebugMetadataHeader!.Id);
                if (content.Guid != codeview.Guid || content.Stamp != debug.Stamp) throw new InvalidOperationException("portable pdb identity differs from executable");
                PdbGuid = content.Guid;Reader = reader;
                using var source = new BinaryReader(File.OpenRead(workload));
                var sourceBytes = source.ReadBytes(4 * 1024 * 1024 + 1);
                if (sourceBytes.Length > 4 * 1024 * 1024) throw new InvalidOperationException("original source exceeds 4 mib checksum limit");
                SourceSha256 = SHA256.HashData(sourceBytes);SourceSha1 = SHA1.HashData(sourceBytes);
                Binding = new { assembly, mvid, pdb_guid = content.Guid, pdb_stamp = content.Stamp, workload,
                    workload_sha256 = Convert.ToHexString(SourceSha256), matched_pdb = true };
            }
            catch (Exception error)
            {
                Reader = null;Warnings.Add("source binding unavailable: " + error.Message);
                Binding = new { assembly, workload, matched_pdb = false, reason = error.Message };
            }
        }

        public (string? File, int? Line, string Meaning) Resolve(string? module, int? token, int ilOffset, Guid? tracePdb)
        {
            if (Reader == null || module == null || Path.GetFullPath(module) != AssemblyPath || token == null || (token.Value & unchecked((int)0xff000000)) != 0x06000000)
                return (null, null, "unresolved");
            if (tracePdb != null && tracePdb != Guid.Empty && tracePdb != PdbGuid) return (null, null, "trace/build mismatch");
            var debug = Reader.GetMethodDebugInformation(MetadataTokens.MethodDefinitionHandle(token.Value & 0x00ffffff));
            SequencePoint? selected = null;
            foreach (var point in debug.GetSequencePoints())
            {
                if (point.IsHidden) continue;
                if (ilOffset < 0) { selected = point;break; }
                if (point.Offset > ilOffset) break;
                selected = point;
            }
            if (selected == null) return (null, null, "no visible sequence point");
            var document = Reader.GetDocument(selected.Value.Document.IsNil ? debug.Document : selected.Value.Document);
            var path = Reader.GetString(document.Name);
            if (Path.GetFileName(path) != "Workload.cs") return (null, null, "bootstrap source excluded");
            var algorithm = Reader.GetGuid(document.HashAlgorithm);
            var digest = algorithm == new Guid("8829d00f-11b8-4213-878b-770e8597ac16") ? SourceSha256
                : algorithm == new Guid("ff1816ec-aa5e-4d10-87f7-6f4963833460") ? SourceSha1 : null;
            if (digest == null || !digest.AsSpan().SequenceEqual(Reader.GetBlobBytes(document.Hash)))
            {
                if (!Warnings.Contains("source checksum mismatch")) Warnings.Add("source checksum mismatch");
                return (null, null, "source checksum mismatch");
            }
            return (Workload, selected.Value.StartLine, ilOffset < 0 ? "matched portable-pdb method association" : "matched portable-pdb il sequence point");
        }

        public void Dispose() => Provider?.Dispose();
    }
}
