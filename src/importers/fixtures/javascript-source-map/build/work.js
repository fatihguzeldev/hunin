export function tsHotspot() {
    const end = Date.now() + 500;
    let total = 0;
    while (Date.now() < end) {
        for (let i = 1; i < 10000; ++i)
            total += Math.sqrt(i);
    }
    return total;
}
console.log(tsHotspot());
//# sourceMappingURL=work.js.map