using System.Runtime.CompilerServices;

public static class Preview
{
    [MethodImpl(MethodImplOptions.NoInlining)]
    public static int[] Resize(int[] input)
    {
        var pixels = new int[input.Length];
        for (var round = 0; round < 150000; round++)
            for (var i = 0; i < input.Length; i++)
                pixels[i] = ((input[i] * 3 + 7) ^ input[i]) & 255;
        return pixels;
    }

    public static object Run()
    {
        var input = Enumerable.Range(0, 256).ToArray();
        int[] pixels = [];
        for (var repetition = 0; repetition < 7; repetition++) pixels = Resize(input);
        return new { pixels, width = 32, height = 8, repetitions = 7 };
    }
}
