"""Synthetic-only live Jev contract check. Never replay chats, memories or traces."""
import argparse, json, sqlite3, time, urllib.request, urllib.error
from pathlib import Path

def main():
    parser=argparse.ArgumentParser();parser.add_argument('--rounds',type=int,default=2);parser.add_argument('--direct',action='store_true');args=parser.parse_args()
    if not 1 <= args.rounds <= 5:parser.error('rounds must be 1..5')
    root=Path(__file__).resolve().parent.parent
    with sqlite3.connect(root/'.local/app.sqlite') as db:
        config=json.loads(db.execute('SELECT payload FROM settings WHERE id=1').fetchone()[0])
    key=config['teamorouter']['api_key']
    opener=urllib.request.build_opener(urllib.request.ProxyHandler({})) if args.direct else urllib.request.build_opener()
    choice={'department':{'type':'choice','instructions':'Which team should handle this customer message?','criteria':{'billing':'Charges, payments and refunds','technical':'Bugs and integration issues','other':'Other requests'}}}
    cases=[('official_sample','I was charged twice for the same order.',choice),('chinese','同一笔订单被扣款两次，请核实。',choice),('structured',{'ticket':'Duplicate charge','history':['Synthetic observation only']*150},choice),('three_primitives','My payouts failed for three days.',dict(choice,urgent={'type':'noul','instructions':'Is the request urgent?'},frustration={'type':'score','instructions':'How frustrated is the customer?','criteria':['Calm','Frustrated','Very angry']}))]
    passed=0;total=0
    for turn in range(args.rounds):
        for name,state,questions in cases:
            body=json.dumps(dict(model='jev',state=state,questions=questions),ensure_ascii=False).encode()
            request=urllib.request.Request('https://api.teamorouter.com/v1/systemone',data=body,headers={'Authorization':'Bearer '+key,'Content-Type':'application/json'})
            started=time.monotonic();status=None;response={};failure=None
            try:
                with opener.open(request,timeout=25) as r:status=r.status;response=json.load(r)
            except urllib.error.HTTPError as e:
                status=e.code
                try:response=json.load(e)
                except ValueError:failure='Non-JSON HTTP error'
            except Exception as e:failure=type(e).__name__
            valid=status==200 and all(q in response.get('answers',{}) for q in questions)
            passed+=int(valid);total+=1
            print(json.dumps(dict(round=turn+1,case=name,status=status,valid_answers=valid,request_bytes=len(body),seconds=round(time.monotonic()-started,2),error=response.get('error',failure),trace_id=response.get('trace_id')),ensure_ascii=False),flush=True)
            time.sleep(0.5)
    print(json.dumps(dict(passed=passed,total=total)),flush=True)
    return 0 if passed==total else 1
if __name__=='__main__':raise SystemExit(main())
