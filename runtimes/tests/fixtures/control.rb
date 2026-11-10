module HuninWorkload
  def self.recurse(n)
    n == 0 ? 0 : recurse(n - 1) + 1
  end
  def self.fail_once
    raise 'expected fixture exception'
  end
  def self.work(n)
    n * 3 + 1
  end
  def self.run
    recursion = recurse(4)
    rescued = false
    begin
      fail_once
    rescue RuntimeError => error
      rescued = error.message == 'expected fixture exception'
    end
    thread = Thread.new { work(9) }
    fiber = Fiber.new { Fiber.yield(work(4)); work(5) }
    {'recursion' => recursion, 'rescued' => rescued, 'thread' => thread.value,
      'fiber_first' => fiber.resume, 'fiber_second' => fiber.resume}
  end
end
