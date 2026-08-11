export function isValidScheduleRange(start: string, stop: string): boolean {
  const parse = (value: string) => {
    if (!/^\d{2}:\d{2}$/.test(value)) return null;
    const [hour, minute] = value.split(':').map(Number);
    return hour <= 23 && minute <= 59 ? hour * 60 + minute : null;
  };
  const startMinutes = parse(start);
  const stopMinutes = parse(stop);
  return startMinutes !== null && stopMinutes !== null && startMinutes !== stopMinutes;
}
