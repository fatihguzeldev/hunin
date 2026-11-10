module HuninWorkload
  def self.resize(data)
    data.each_with_index.map { |value, index| ((value * 3 + 7) ^ (index & 255)) & 255 }
  end

  def self.run
    data = (0...4096).map { |i| i & 255 }
    output = nil
    7.times { output = resize(data) }
    {'width' => 64, 'height' => 64, 'batches' => 7, 'pixels' => output}
  end
end
