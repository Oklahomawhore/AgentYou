import unittest
from github_weekly import parse

class WeeklyParserTest(unittest.TestCase):
    def test_fields_entities_and_weekly_stars(self):
        page='''<article><h2><a href="/owner/project"><span>owner /</span> project</a></h2><p>Tools &amp; ideas</p><span itemprop="programmingLanguage">Rust</span><a>99,999</a><span>1,234 stars this week</span></article>'''
        item=parse(page)[0]
        self.assertEqual(item['repository'],'owner/project')
        self.assertEqual(item['description'],'Tools & ideas')
        self.assertEqual(item['language'],'Rust')
        self.assertEqual(item['stars_this_week'],1234)
        self.assertEqual(len(parse(page+page)),1)
    def test_missing_fields_and_non_repository_links(self):
        items=parse('<article><h2><a href="/a/b">b</a></h2></article><article><h2><a href="https://evil.example/a/b">bad</a></h2></article>')
        self.assertEqual(len(items),1)
        self.assertIsNone(items[0]['stars_this_week'])
        self.assertIsNone(items[0]['language'])
        self.assertEqual(parse('<html>Rate limited</html>'),[])

if __name__=='__main__':unittest.main()
