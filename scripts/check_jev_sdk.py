"""Official SDK live probe; synthetic inputs only, secrets never printed."""
import json,sqlite3,time
from pathlib import Path
from importlib.metadata import version
from typesafe_sdk import Choice,TypeSafeClient

def main():
    with sqlite3.connect(Path(__file__).resolve().parent.parent/'.local/app.sqlite') as db:
        key=json.loads(db.execute('SELECT payload FROM settings WHERE id=1').fetchone()[0])['teamorouter']['api_key']
    print('typesafe-sdk',version('typesafe-sdk'),flush=True)
    passed=0
    with TypeSafeClient(api_key=key,base_url='https://api.teamorouter.com',model='jev',timeout=10) as client:
        for label,state in [('official_english','I was charged twice for the same order.'),('chinese','同一笔订单被扣款两次，请核实。')]:
            started=time.monotonic()
            try:
                r=client.system_one(state=state,questions={'department':Choice(instructions='Which team should handle this customer message?',criteria={'billing':'Charges, payments and refunds','technical':'Bugs and integration issues','other':'Other requests'})})
                answer=r.answers['department'];passed+=1
                print(json.dumps({'case':label,'ok':True,'answer':answer.model_dump() if hasattr(answer,'model_dump') else str(answer),'seconds':round(time.monotonic()-started,2)},ensure_ascii=False),flush=True)
            except Exception as e:
                body=getattr(e,'body',None)
                print(json.dumps({'case':label,'ok':False,'exception':type(e).__name__,'status':getattr(e,'status',None),'body':body,'seconds':round(time.monotonic()-started,2)},ensure_ascii=False),flush=True)
    print('successful_calls',passed,'/ 2',flush=True)
    return 0 if passed==2 else 1
if __name__=='__main__':raise SystemExit(main())
