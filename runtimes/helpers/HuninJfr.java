import java.io.*;
import java.lang.reflect.*;
import java.nio.file.*;
import java.security.MessageDigest;
import java.time.*;
import java.util.*;
import jdk.jfr.*;
import jdk.jfr.consumer.*;

public class HuninJfr {
    record SourceBinding(String filename,String sha256) {}
    static final Map<Long,SourceBinding> classFiles=new HashMap<>();
    static final Map<String,String> expectedClasses=new LinkedHashMap<>();
    static void json(Object value, StringBuilder out) {
        if (value == null) { out.append("null"); return; }
        if (value instanceof String text) {
            out.append('"');
            for (int i=0;i<text.length();i++) {
                char c=text.charAt(i);
                switch(c) {
                    case '"': out.append("\\\""); break;
                    case '\\': out.append("\\\\"); break;
                    case '\n': out.append("\\n"); break;
                    case '\r': out.append("\\r"); break;
                    case '\t': out.append("\\t"); break;
                    default: if(c<32)out.append(String.format("\\u%04x",(int)c));else out.append(c);
                }
            }
            out.append('"');
        } else if (value instanceof Number || value instanceof Boolean) out.append(value);
        else if (value instanceof Map<?,?> map) {
            out.append('{'); boolean comma=false;
            for(var entry:map.entrySet()) { if(comma)out.append(',');comma=true;json(entry.getKey().toString(),out);out.append(':');json(entry.getValue(),out); }
            out.append('}');
        } else if (value instanceof Iterable<?> sequence) {
            out.append('['); boolean comma=false;
            for(var item:sequence) { if(comma)out.append(',');comma=true;json(item,out); }out.append(']');
        } else if (value.getClass().isArray()) {
            out.append('[');for(int i=0;i<Array.getLength(value);i++){if(i>0)out.append(',');json(Array.get(value,i),out);}out.append(']');
        } else throw new IllegalArgumentException("unsupported prepared output type: "+value.getClass());
    }
    static void write(Path path,Object value) throws Exception {
        StringBuilder out=new StringBuilder();json(value,out);
        Files.writeString(path,out.toString(),StandardOpenOption.CREATE_NEW);
        try(var channel=java.nio.channels.FileChannel.open(path,StandardOpenOption.WRITE)){channel.force(true);}
    }
    static Map<String,Object> values(Object... fields) {
        Map<String,Object> result=new LinkedHashMap<>();for(int i=0;i<fields.length;i+=2)result.put((String)fields[i],fields[i+1]);return result;
    }
    static void skipAttributes(DataInputStream in) throws Exception {
        int n=in.readUnsignedShort();for(int i=0;i<n;i++){in.readUnsignedShort();in.skipNBytes(Integer.toUnsignedLong(in.readInt()));}
    }
    static String sourceFile(byte[] bytes) throws Exception {
        DataInputStream in=new DataInputStream(new ByteArrayInputStream(bytes));
        if(in.readInt()!=0xcafebabe)throw new IOException("invalid class magic");in.skipNBytes(4);
        int count=in.readUnsignedShort();String[] strings=new String[count];
        for(int i=1;i<count;i++) {
            switch(in.readUnsignedByte()) {
                case 1: strings[i]=in.readUTF();break;
                case 3:case 4:in.skipNBytes(4);break;
                case 5:case 6:in.skipNBytes(8);i++;break;
                case 7:case 8:case 16:case 19:case 20:in.skipNBytes(2);break;
                case 9:case 10:case 11:case 12:case 17:case 18:in.skipNBytes(4);break;
                case 15:in.skipNBytes(3);break;
                default:throw new IOException("unsupported class constant tag");
            }
        }
        in.skipNBytes(6);int interfaces=in.readUnsignedShort();in.skipNBytes(interfaces*2L);
        for(int section=0;section<2;section++){int n=in.readUnsignedShort();for(int i=0;i<n;i++){in.skipNBytes(6);skipAttributes(in);}}
        int n=in.readUnsignedShort();for(int i=0;i<n;i++){int name=in.readUnsignedShort();long length=Integer.toUnsignedLong(in.readInt());if("SourceFile".equals(strings[name])&&length==2)return strings[in.readUnsignedShort()];in.skipNBytes(length);}
        return null;
    }
    static List<Object> frames(RecordedStackTrace stack,String workload,Path source) throws Exception {
        List<Object> result=new ArrayList<>();if(stack==null)return result;
        for(RecordedFrame frame:stack.getFrames()) {
            RecordedMethod method=frame.getMethod();RecordedClass type=method.getType();
            String sourceName=null,digest=null;RecordedClassLoader classLoader=type.getClassLoader();
            String loader=classLoader==null?null:classLoader.getName();
            String loaderType=classLoader==null||classLoader.getType()==null?null:classLoader.getType().getName();
            // Bind only the prepared class loaded by the ordinary application loader.
            if((type.getName().equals(workload)||type.getName().startsWith(workload+"$"))&&"app".equals(loader)&&"jdk.internal.loader.ClassLoaders$AppClassLoader".equals(loaderType)) {
                SourceBinding binding=classFiles.get(type.getId());
                if(binding==null) {
                    String resource=type.getName().replace('.','/')+".class";
                    try(InputStream stream=ClassLoader.getSystemResourceAsStream(resource)) {
                        if(stream!=null){byte[] bytes=stream.readNBytes(1_048_577);if(bytes.length<=1_048_576)binding=new SourceBinding(sourceFile(bytes),HexFormat.of().formatHex(MessageDigest.getInstance("SHA-256").digest(bytes)));}
                    }
                    if(binding==null)binding=new SourceBinding(null,null);
                    classFiles.put(type.getId(),binding);
                }
                sourceName=binding.filename();digest=binding.sha256();
                if(!Objects.equals(digest,expectedClasses.get(type.getName())))sourceName=null;
                if(sourceName!=null&&!source.getFileName().toString().equals(sourceName))sourceName=null;
            }
            result.add(values("class",type.getName(),"class_id",type.getId(),"loader",loader,"loader_id",classLoader==null?null:classLoader.getId(),"loader_type",loaderType,
                "method",method.getName(),"descriptor",method.getDescriptor(),"line",frame.getLineNumber(),
                "method_modifiers",method.getModifiers(),"method_hidden",method.isHidden(),
                "bci",frame.getBytecodeIndex(),"frame_type",frame.getType(),"java_frame",frame.isJavaFrame(),
                "source",sourceName==null?null:source.toString(),"class_file_sha256",digest,
                "expected_class_file_sha256",expectedClasses.get(type.getName())));
        }
        return result;
    }
    public static void main(String[] args) throws Exception {
        String workload=args[0];Path source=Path.of(args[1]),raw=Path.of(args[2]),profile=Path.of(args[3]),report=Path.of(args[4]);
        String sourceHash=null;
        for(String line:Files.readAllLines(Path.of(args[5]))) {
            String[] fields=line.split(" ");
            if(fields.length==2&&fields[0].equals("source"))sourceHash=fields[1];
            else if(fields.length==3&&fields[0].equals("class"))expectedClasses.put(fields[1],fields[2]);
            else throw new IOException("invalid prepared class manifest");
        }
        BufferedReader commands=new BufferedReader(new InputStreamReader(System.in));
        Object output=null;String outcome="stopped_before_work",failure=null;Long workStart=null,workEnd=null;
        try(Recording recording=new Recording()) {
            recording.enable("jdk.ExecutionSample").withPeriod(Duration.ofMillis(10)).withStackTrace();
            recording.start();long readyNs=System.nanoTime();
            if(recording.getState()!=RecordingState.RUNNING)throw new IllegalStateException("recording did not start");
            System.out.println("HUNIN_READY");System.out.flush();String command=commands.readLine();
            try {
                if("HUNIN_GO".equals(command)) {
                    workStart=System.nanoTime();output=Class.forName(workload).getMethod("run").invoke(null);outcome="completed";
                } else if(command!=null&&!"HUNIN_STOP".equals(command))throw new IllegalArgumentException("invalid gate command");
            } catch(Throwable error){outcome="application_failed";Throwable cause=error instanceof InvocationTargetException&&error.getCause()!=null?error.getCause():error;failure=cause.toString();}
            finally{if(workStart!=null)workEnd=System.nanoTime();}
            recording.stop();recording.dump(raw);
            List<Object> samples=new ArrayList<>();Map<String,Long> unsupported=new TreeMap<>();long lost=0;
            try(RecordingFile file=new RecordingFile(raw)) {
                while(file.hasMoreEvents()) {
                    RecordedEvent event=file.readEvent();String kind=event.getEventType().getName();
                    if(!kind.equals("jdk.ExecutionSample")){unsupported.merge(kind,1L,Long::sum);continue;}
                    if(samples.size()>=100_000){lost++;continue;}
                    RecordedThread thread=event.getValue("sampledThread");RecordedStackTrace stack=event.getStackTrace();
                    Instant time=event.getStartTime();long epochNs=Math.addExact(Math.multiplyExact(time.getEpochSecond(),1_000_000_000L),time.getNano());
                    samples.add(values("timestamp_ns",epochNs,"duration_ns",event.getDuration().toNanos(),
                        "event_type",kind,"thread_java_id",thread==null?null:thread.getJavaThreadId(),
                        "thread_os_id",thread==null?null:thread.getOSThreadId(),"thread_name",thread==null?null:thread.getJavaName(),
                        "state",event.getString("state"),"truncated",stack!=null&&stack.isTruncated(),
                        "frames",frames(stack,workload,source)));
                }
            }
            write(profile,values("runtime","hotspot","version",System.getProperty("java.runtime.version"),
                "vm_name",System.getProperty("java.vm.name"),"vm_version",System.getProperty("java.vm.version"),
                "vm_arguments",java.lang.management.ManagementFactory.getRuntimeMXBean().getInputArguments(),"settings",recording.getSettings(),
                "pid",ProcessHandle.current().pid(),"period_ns",10_000_000,"clock","jdk RecordingFile Instant conversion; no helper-clock alignment assumed",
                "samples",samples,"lost_samples",lost,"unsupported_event_types",unsupported,
                "source_binding","prepared app-loader class resource SourceFile and class-file hash; loaded-byte/redefinition identity is not observed",
                "expected_classes",expectedClasses,"source_sha256",sourceHash,
                "omitted_fields",List.of("thread group ancestry","unselected JFR event payloads","JFR binary constant-pool layout")));
            write(report,values("outcome",outcome,"output",output,"application_error",failure,"helper_clock","System.nanoTime",
                "source_ready_ns",readyNs,"work_start_ns",workStart,"work_end_ns",workEnd));
        }
        if("1".equals(System.getenv("HUNIN_FINALIZE_GATE"))){System.out.println("HUNIN_WORK_COMPLETE");System.out.flush();String finish=commands.readLine();if(finish!=null&&!finish.equals("HUNIN_EXIT")&&!finish.equals("HUNIN_STOP"))throw new IllegalArgumentException("invalid finalize command");}
        if(failure!=null)System.exit(1);
    }
}
