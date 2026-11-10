using System.Security.Cryptography;
using System.Runtime.CompilerServices;
using System.Text;
using System.Text.Json;

public static class AssetPreview
{
    static bool White(byte value) => value is 9 or 10 or 11 or 12 or 13 or 32;

    static string Token(byte[] bytes, ref int position)
    {
        while (position < bytes.Length)
        {
            if (bytes[position] == '#')
                while (position < bytes.Length && bytes[position] != '\n') position++;
            else if (White(bytes[position])) position++;
            else break;
        }
        var start = position;
        while (position < bytes.Length && !White(bytes[position])) position++;
        if (position == start) throw new InvalidDataException("missing ppm header token");
        return Encoding.ASCII.GetString(bytes, start, position - start);
    }

    [MethodImpl(MethodImplOptions.NoInlining)]
    static byte[] Resize(byte[] source, int start, int width, int height)
    {
        var header = Encoding.ASCII.GetBytes("P6\n32 32\n255\n");
        var output = new byte[header.Length + 32 * 32 * 3];
        header.CopyTo(output, 0);
        for (var y = 0; y < 32; y++)
            for (var x = 0; x < 32; x++)
            {
                var sourcePosition = start + ((y * height / 32) * width + x * width / 32) * 3;
                var destination = header.Length + (y * 32 + x) * 3;
                Array.Copy(source, sourcePosition, output, destination, 3);
            }
        return output;
    }

    public static object Run()
    {
        var input = Environment.GetEnvironmentVariable("PREVIEW_INPUT") ?? throw new InvalidOperationException("PREVIEW_INPUT is required");
        var output = Environment.GetEnvironmentVariable("PREVIEW_OUTPUT") ?? throw new InvalidOperationException("PREVIEW_OUTPUT is required");
        var repetitions = int.Parse(Environment.GetEnvironmentVariable("PREVIEW_REPETITIONS") ?? "1");
        if (repetitions < 1) throw new InvalidOperationException("positive preview repetitions are required");
        Directory.CreateDirectory(output);
        var assets = new List<object>();
        foreach (var sourcePath in Directory.GetFiles(input, "*.ppm").OrderBy(path => Path.GetFileName(path), StringComparer.Ordinal))
        {
            var source = File.ReadAllBytes(sourcePath);var position = 0;
            var magic = Token(source, ref position);
            var width = int.Parse(Token(source, ref position));
            var height = int.Parse(Token(source, ref position));
            var maximum = Token(source, ref position);
            if (magic != "P6" || maximum != "255" || width < 1 || height < 1 || width > 65536 || height > 65536)
                throw new InvalidDataException("unsupported ppm input");
            if (position >= source.Length || !White(source[position])) throw new InvalidDataException("missing ppm payload separator");
            position += source[position] == '\r' && position + 1 < source.Length && source[position + 1] == '\n' ? 2 : 1;
            if ((long)source.Length - position != (long)width * height * 3) throw new InvalidDataException("ppm payload length differs from dimensions");
            byte[] preview = [];
            for (var repetition = 0; repetition < repetitions; repetition++) preview = Resize(source, position, width, height);
            var name = Path.GetFileName(sourcePath);var destination = Path.GetFileNameWithoutExtension(name) + ".preview.ppm";
            File.WriteAllBytes(Path.Combine(output, destination), preview);
            assets.Add(new { input = name, output = destination, source_width = width, source_height = height,
                bytes = preview.Length, sha256 = Convert.ToHexString(SHA256.HashData(preview)).ToLowerInvariant() });
        }
        var manifest = new { algorithm = "nearest-neighbor-rgb8", width = 32, height = 32, assets };
        File.WriteAllText(Path.Combine(output, "manifest.json"), JsonSerializer.Serialize(manifest), new UTF8Encoding(false));
        return manifest;
    }
}
