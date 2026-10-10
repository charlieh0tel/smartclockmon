#!/bin/sh
# Load each HTML page under docs/ in headless Chrome at desktop widths and
# fail if the page is wider than the window or any box on it scrolls
# sideways.  Headless screenshots hide scrollbars, so this measures
# instead of looking.  Needs google-chrome or chromium; not run in CI.
set -eu

WIDTHS="1920 1280 1000"

chrome=$(command -v google-chrome || command -v chromium || command -v chromium-browser || true)
if [ -z "$chrome" ]; then
    echo "check-pages: no google-chrome or chromium found" >&2
    exit 2
fi

scratch=$(mktemp -d)
trap 'rm -rf "$scratch"' EXIT

# Reports, after load, the page's overflow and each element allowed to
# scroll sideways whose content is wider than it.
probe='<script>addEventListener("load",()=>setTimeout(()=>{const bad=[];
if(document.documentElement.scrollWidth>innerWidth+1)bad.push("page "+document.documentElement.scrollWidth+">"+innerWidth);
for(const e of document.querySelectorAll("*")){const o=getComputedStyle(e).overflowX;
if((o==="auto"||o==="scroll")&&e.scrollWidth>e.clientWidth+1)bad.push((e.id||e.className||e.tagName)+" "+e.scrollWidth+">"+e.clientWidth);}
const p=document.createElement("pre");p.id="check-pages";p.textContent=bad.length?bad.join("; "):"ok";document.body.appendChild(p);},300));</script>'

status=0
for page in $(find docs -name "*.html" | sort); do
    copy="$scratch/$(basename "$page")"
    # By position, not sub(): the probe holds "&", which sub() expands.
    PROBE="$probe" awk '{ i = index($0, "</body>"); if (i) $0 = substr($0, 1, i - 1) ENVIRON["PROBE"] substr($0, i); print }' "$page" > "$copy"
    for width in $WIDTHS; do
        result=$("$chrome" --headless=new --disable-gpu --window-size="$width,1000" \
            --virtual-time-budget=4000 --dump-dom "file://$copy" 2>/dev/null \
            | sed -n 's/.*<pre id="check-pages">\(.*\)<\/pre>.*/\1/p' | sed 's/&gt;/>/g; s/&lt;/</g; s/&amp;/\&/g')
        if [ "$result" = "ok" ]; then
            echo "ok    $page at $width"
        else
            echo "FAIL  $page at $width: ${result:-no result}"
            status=1
        fi
    done
done
exit $status
