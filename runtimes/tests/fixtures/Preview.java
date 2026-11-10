import java.util.Map;

public class Preview {
    public static int[] resize(int[] input) {
        int[] output=new int[input.length];
        for(int i=0;i<input.length;i++)output[i]=((input[i]*3+7)^(i&255))&255;
        return output;
    }
    public static Object run() {
        int[] input=new int[65536];for(int i=0;i<input.length;i++)input[i]=i&255;
        int[] output=null;for(int batch=0;batch<4096;batch++)output=resize(input);
        return Map.of("width",256,"height",256,"batches",4096,"pixels",output);
    }
}
