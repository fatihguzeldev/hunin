import java.nio.file.*;
import java.nio.charset.StandardCharsets;
import java.security.MessageDigest;
import java.util.*;

public class AssetPreview {
    record Image(int width,int height,byte[] pixels) {}
    static boolean space(int c){return c==9||c==10||c==13||c==32;}
    static Image readImage(byte[] data) {
        int[] cursor={0};
        java.util.function.Supplier<String> token=()->{
            while(cursor[0]<data.length){
                if(data[cursor[0]]==35){while(cursor[0]<data.length&&data[cursor[0]]!=10)cursor[0]++;}
                else if(space(data[cursor[0]]))cursor[0]++;
                else break;
            }
            int start=cursor[0];while(cursor[0]<data.length&&!space(data[cursor[0]])&&data[cursor[0]]!=35)cursor[0]++;
            return new String(data,start,cursor[0]-start,StandardCharsets.US_ASCII);
        };
        if(!token.get().equals("P6"))throw new IllegalArgumentException("expected P6 RGB image");
        int width=Integer.parseInt(token.get()),height=Integer.parseInt(token.get()),maximum=Integer.parseInt(token.get());
        if(width<1||width>4096||height<1||height>4096||maximum!=255)throw new IllegalArgumentException("invalid RGB8 dimensions");
        int position=cursor[0];
        if(position+1<data.length&&data[position]==13&&data[position+1]==10)position+=2;
        else if(position<data.length&&space(data[position]))position++;
        else throw new IllegalArgumentException("missing PPM pixel delimiter");
        byte[] pixels=Arrays.copyOfRange(data,position,data.length);
        if(pixels.length!=width*height*3)throw new IllegalArgumentException("unexpected PPM pixel length");
        return new Image(width,height,pixels);
    }
    static byte[] resize(Image image) {
        byte[] output=new byte[32*32*3];
        for(int y=0;y<32;y++)for(int x=0;x<32;x++){
            int from=((y*image.height()/32)*image.width()+x*image.width()/32)*3,to=(y*32+x)*3;
            System.arraycopy(image.pixels(),from,output,to,3);
        }
        return output;
    }
    static String json(Object value) {
        if(value instanceof String text){
            StringBuilder out=new StringBuilder("\"");
            for(char c:text.toCharArray()){if(c=='"'||c=='\\')out.append('\\').append(c);else if(c<32)out.append(String.format("\\u%04x",(int)c));else out.append(c);}
            return out.append('"').toString();
        }
        if(value instanceof Number||value instanceof Boolean)return value.toString();
        if(value instanceof Map<?,?> values){StringJoiner out=new StringJoiner(",","{","}");for(var entry:values.entrySet())out.add(json(entry.getKey())+":"+json(entry.getValue()));return out.toString();}
        if(value instanceof Iterable<?> values){StringJoiner out=new StringJoiner(",","[","]");for(var item:values)out.add(json(item));return out.toString();}
        throw new IllegalArgumentException("unsupported manifest value");
    }
    public static Object run() throws Exception {
        Path input=Path.of(System.getenv("PREVIEW_INPUT")),output=Path.of(System.getenv("PREVIEW_OUTPUT"));
        int repetitions=Integer.parseInt(System.getenv().getOrDefault("PREVIEW_REPETITIONS","50000"));
        if(!input.isAbsolute()||!output.isAbsolute()||repetitions<1||repetitions>100000)throw new IllegalArgumentException("invalid preview input/output/repetitions");
        Files.createDirectories(output);List<Object> assets=new ArrayList<>();
        try(var paths=Files.list(input)){
            for(Path path:paths.filter(p->p.getFileName().toString().endsWith(".ppm")).sorted().toList()){
                Image image=readImage(Files.readAllBytes(path));byte[] pixels=null;
                for(int pass=0;pass<repetitions;pass++)pixels=resize(image);
                byte[] header="P6\n32 32\n255\n".getBytes(StandardCharsets.US_ASCII),data=new byte[header.length+pixels.length];
                System.arraycopy(header,0,data,0,header.length);System.arraycopy(pixels,0,data,header.length,pixels.length);
                String name=path.getFileName().toString(),filename=name.substring(0,name.length()-4)+".preview.ppm";
                Files.write(output.resolve(filename),data);
                assets.add(Map.of("input",name,"output",filename,"source_width",image.width(),"source_height",image.height(),"bytes",data.length,"sha256",HexFormat.of().formatHex(MessageDigest.getInstance("SHA-256").digest(data))));
            }
        }
        if(assets.isEmpty())throw new IllegalArgumentException("preview directory has no PPM assets");
        Map<String,Object> manifest=Map.of("algorithm","nearest-neighbor-rgb8","width",32,"height",32,"assets",assets);
        Files.writeString(output.resolve("manifest.json"),json(manifest));return manifest;
    }
}
