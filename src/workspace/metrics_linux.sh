LC_ALL=C; export LC_ALL
if [ "$(uname -s)" != Linux ]; then echo 'TSHELL_UNSUPPORTED'; exit; fi
system=$(uname -sr)
arch=$(uname -m)
cores=$(getconf _NPROCESSORS_ONLN)
model=$(sed -n 's/^model name[[:space:]]*: //p' /proc/cpuinfo | head -n 1)
release=$(sed -n 's/^PRETTY_NAME=//p' /etc/os-release 2>/dev/null | tr -d '\042')
tick=0
disks=
while :; do
printf 'TSHELL_METRICS_BEGIN\nos %s\narch %s\ncores %s\nmodel %s\n' "$system" "$arch" "$cores" "$model"
[ "$collect_cpu" = 0 ] || cat /proc/stat
printf 'uptime '; cat /proc/uptime
printf 'release %s\n' "$release"
if [ "$collect_network" = 1 ]; then
primary=$(awk '$2 == "00000000" && $4 ~ /[13579bBdDfF]$/ {if (!found || $7+0 < metric) {name=$1; metric=$7+0; found=1}} END {print name}' /proc/net/route)
if [ -z "$primary" ]; then primary=$(awk '$2 == "00" && $1 == "00000000000000000000000000000000" && $10 != "lo" {print $10; exit}' /proc/net/ipv6_route); fi
printf 'primary %s\n' "$primary"
awk -F '[: ]+' '/:/ { sub(/^[ \t]+/, ""); n=split($0,a,/[: \t]+/); if(n>=17) printf "net:%s %s %s\n",a[1],a[2],a[10] }' /proc/net/dev
fi
if [ "$collect_io" = 1 ]; then
awk '{printf "io:%s %s %s %s %s\n",$3,$4,$6,$8,$10}' /proc/diskstats
# Whole leaf block devices: partitions and stacked md/dm devices must not be summed twice.
for disk in /sys/block/*; do
  name=${disk##*/}
  case "$name" in loop*|ram*|zram*) continue;; esac
  [ -d "$disk" ] || continue
  set -- "$disk"/slaves/*
  [ -e "$1" ] || printf 'iototal %s\n' "$name"
done
fi
if [ "$collect_temperature" = 1 ]; then
for hw in /sys/class/hwmon/hwmon*; do
  [ -r "$hw/name" ] || continue
  chip=$(cat "$hw/name")
  for sensor in "$hw"/temp*_input; do
    [ -r "$sensor" ] || continue
    label=${sensor%_input}_label
    if [ -r "$label" ]; then label=$(cat "$label"); else label=${sensor##*/}; fi
    printf 'temp %s %s %s\n' "$chip" "$label" "$(cat "$sensor")"
  done
done
for zone in /sys/class/thermal/thermal_zone*; do
  if [ -r "$zone/temp" ] && [ -r "$zone/type" ]; then printf 'temp %s %s\n' "$(cat "$zone/type")" "$(cat "$zone/temp")"; fi
done
fi
# Capacity changes slowly; reuse the last sample for six fast sampling cycles.
if [ "$collect_disk" = 1 ]; then
  if [ "$tick" = 0 ]; then
    disks=$(df -Plk -x tmpfs -x devtmpfs -x squashfs 2>/dev/null | awk 'NR>1 {print "mount " $0; if ($6 == "/") print "disk " $0}')
  fi
  printf '%s\n' "$disks"
fi
[ "$collect_memory" = 0 ] || cat /proc/meminfo
tick=$(( (tick + 1) % 6 ))
printf 'TSHELL_METRICS_END\n'
sleep 5
done
