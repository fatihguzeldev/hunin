require 'digest'
module HuninWorkload
  def self.read_image(data)
    cursor = 0
    token = lambda do
      while cursor < data.bytesize
        if data.getbyte(cursor) == 35
          cursor += 1 while cursor < data.bytesize && data.getbyte(cursor) != 10
        elsif [9,10,13,32].include?(data.getbyte(cursor))
          cursor += 1
        else
          break
        end
      end
      start = cursor
      cursor += 1 while cursor < data.bytesize && ![9,10,13,32,35].include?(data.getbyte(cursor))
      data.byteslice(start,cursor-start)
    end
    raise 'expected P6 RGB image' unless token.call == 'P6'
    width, height, maximum = [Integer(token.call),Integer(token.call),Integer(token.call)]
    raise 'invalid RGB8 dimensions' unless (1..4096).cover?(width) && (1..4096).cover?(height) && maximum == 255
    if data.byteslice(cursor,2) == "\r\n"
      cursor += 2
    elsif [9,10,13,32].include?(data.getbyte(cursor))
      cursor += 1
    else
      raise 'missing PPM pixel delimiter'
    end
    pixels = data.byteslice(cursor..-1)
    raise 'unexpected PPM pixel length' unless pixels.bytesize == width * height * 3
    [width,height,pixels]
  end
  def self.resize(image)
    width,height,source = image
    output = "\0".b * (32*32*3)
    32.times do |y|
      32.times do |x|
        from = ((y*height/32)*width + x*width/32)*3
        dest = (y*32+x)*3
        output.setbyte(dest,source.getbyte(from))
        output.setbyte(dest+1,source.getbyte(from+1))
        output.setbyte(dest+2,source.getbyte(from+2))
      end
    end
    output
  end
  def self.run
    input,output = ENV.fetch('PREVIEW_INPUT'),ENV.fetch('PREVIEW_OUTPUT')
    repetitions = Integer(ENV.fetch('PREVIEW_REPETITIONS','32'))
    raise 'invalid preview input/output/repetitions' unless File.absolute_path?(input) && File.absolute_path?(output) && (1..100000).cover?(repetitions)
    Dir.mkdir(output) unless Dir.exist?(output)
    assets = Dir.children(input).select { |name| name.end_with?('.ppm') }.sort.map do |name|
      image = read_image(File.binread(File.join(input,name)))
      pixels = nil
      repetitions.times { pixels = resize(image) }
      data = "P6\n32 32\n255\n".b + pixels
      filename = name[0...-4] + '.preview.ppm'
      File.binwrite(File.join(output,filename),data)
      {'input'=>name,'output'=>filename,'source_width'=>image[0],'source_height'=>image[1],'bytes'=>data.bytesize,'sha256'=>Digest::SHA256.hexdigest(data)}
    end
    raise 'preview directory has no PPM assets' if assets.empty?
    manifest = {'algorithm'=>'nearest-neighbor-rgb8','width'=>32,'height'=>32,'assets'=>assets}
    File.write(File.join(output,'manifest.json'),JSON.generate(manifest))
    manifest
  end
end
