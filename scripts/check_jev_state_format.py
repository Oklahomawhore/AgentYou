"""Compare object and Markdown states with synthetic data only; no agent actions."""
import json, sqlite3, time, urllib.request, urllib.error
from pathlib import Path

def markdown(v, depth=0):
    if isinstance(v,dict):
        return '\n'.join('  '*depth+'- '+k+':\n'+markdown(x,depth+1) for k,x in v.items())
    if isinstance(v,list):
        return '\n'.join('  '*depth+'- ['+str(i)+']:\n'+markdown(x,depth+1) for i,x in enumerate(v))
    return '  '*depth+json.dumps(v,ensure_ascii=False)

root=Path(__file__).resolve().parent.parent
with sqlite3.connect(root/'.local/app.sqlite') as db:
    config=json.loads(db.execute('SELECT payload FROM settings WHERE id=1').fetchone()[0])
key=config['teamorouter']['api_key']
# All observations below are fictional, unrelated to this user's actual context.
state={'current_time':'2026-01-02T10:00:00+08:00','persona':{'core':{'description':'A fictional assistant who values accuracy and user attention.'}},'memory':{'long_term':{'user_preferences':['Prefers short answers.']},'medium_term':{'goal':'Help organize a fictional book club.'},'short_term':{'goal':'Answer the latest question.'}},'recent_interactions':[{'time':'2026-01-02T09:59:00+08:00','role':'user','content':'Please confirm whether the fictional meeting is tomorrow at 14:00.'}],'evidence':{'calendar':{'meeting':{'date':'2026-01-03','time':'14:00','verified':True}}}}
questions={'action':{'type':'choice','instructions':'Choose the next action using the context.','criteria':{'reply':'A direct answer to the user is available.','organize':'Need internal work before answering.','wait':'No useful response or internal work is needed.'}},'meeting_time':{'type':'choice','instructions':'Does the available evidence support the proposed meeting time?','criteria':{'confirmed':'Tomorrow at 14:00 is supported.','different':'The meeting time is different.','unknown':'Evidence is insufficient.'}}}
# Match a realistic context size with explicitly fictional historical records.
state['history']=[{'time':f'2026-01-01T{8+i%12:02d}:00:00+08:00','observation':{'topic':'Fictional reading notes','text':'This is synthetic background about a book club. It does not alter the current meeting time.'}} for i in range(80)]
variants={'object':state,'markdown':'# Fictional context\n'+markdown(state)}
rows=[]
for round_no,order in enumerate([['object','markdown'],['markdown','object'],['object','markdown']],1):
    for name in order:
        body=json.dumps({'model':'jev','state':variants[name],'questions':questions},ensure_ascii=False,separators=(',',':')).encode()
        req=urllib.request.Request('https://api.teamorouter.com/v1/systemone',data=body,headers={'Authorization':'Bearer '+key,'Content-Type':'application/json'})
        start=time.monotonic();code=None;response={};error=None
        try:
            with urllib.request.urlopen(req,timeout=30) as r:code=r.status;response=json.load(r)
        except urllib.error.HTTPError as e:
            code=e.code
            try:response=json.load(e)
            except ValueError:error='non_json'
        except Exception as e:error=type(e).__name__
        answers=response.get('answers',{})
        valid=code==200 and all(isinstance(answers.get(q,{}).get('probabilities'),dict) for q in questions)
        result={'round':round_no,'format':name,'http':code,'valid':valid,'bytes':len(body),'seconds':round(time.monotonic()-start,2),'answers':answers,'error':error or ('upstream_error' if response.get('error') else None)}
        rows.append(result);print(json.dumps(result,ensure_ascii=False),flush=True)
(root/'.local/jev-format-synthetic.json').write_text(json.dumps(rows,ensure_ascii=False,indent=2))
