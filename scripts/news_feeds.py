"""Public RSS only: no account, API key, conversation or memory sent to feeds."""
import concurrent.futures, datetime, email.utils, json, urllib.request, xml.etree.ElementTree as ET
SOURCES = [
    ('中国新闻网', 'https://www.chinanews.com.cn/rss/scroll-news.xml'),
    ('BBC World', 'https://feeds.bbci.co.uk/news/world/rss.xml'),
    ('The Guardian World', 'https://www.theguardian.com/world/rss'),
]
def fetch(source):
    name, url = source
    try:
        request = urllib.request.Request(url, headers={'User-Agent': 'YourSelf-RSS/1.0'})
        with urllib.request.urlopen(request, timeout=12) as response:
            raw = response.read(1_000_001)
        if len(raw) > 1_000_000: raise ValueError('feed too large')
        root = ET.fromstring(raw)
        now = datetime.datetime.now(datetime.timezone.utc)
        items = []
        for entry in root.findall('.//item'):
            title = (entry.findtext('title') or '').strip()[:240]
            link = (entry.findtext('link') or '').strip()
            published = entry.findtext('pubDate') or ''
            try:
                date = email.utils.parsedate_to_datetime(published)
                if date.tzinfo is None: date = date.replace(tzinfo=datetime.timezone.utc)
                age = (now-date).total_seconds()
                if age < -3600 or age > 172800: continue
            except (ValueError, TypeError, OverflowError): continue
            if title and link.startswith(('https://', 'http://')):
                items.append(dict(title=title, url=link, source=name, published_at=date.isoformat()))
        items.sort(key=lambda x:x['published_at'], reverse=True)
        return dict(source=name, feed=url, items=items[:10], error=None if items else 'No dated items in the last 48 hours')
    except Exception as error:
        return dict(source=name, feed=url, items=[], error=type(error).__name__+': '+str(error)[:200])
if __name__ == '__main__':
    with concurrent.futures.ThreadPoolExecutor(max_workers=3) as pool:
        print(json.dumps(list(pool.map(fetch, SOURCES)), ensure_ascii=False))
