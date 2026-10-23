







































type Payload = {value: number};
export function tsHotspot(): number {
  const end: number = Date.now() + 500;
  let total: number = 0;
  while (Date.now() < end) {
    for (let i: number = 1; i < 10000; ++i) total += Math.sqrt(i);
  }
  return total;
}
console.log(tsHotspot());
