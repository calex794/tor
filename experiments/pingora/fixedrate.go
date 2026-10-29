// Isolated, fixed-arrival HTTP/1.1 probe. Standard library only.
package main

import (
 "context"
 "encoding/json"
 "flag"
 "fmt"
 "io"
 "net"
 "net/http"
 "os"
 "sort"
 "sync"
 "sync/atomic"
 "time"
)

type Sample struct { LatencyMicros int64; Status int; Error bool }

func percentile(sorted []int64, p int) float64 {
 if len(sorted) == 0 { return 0 }
 at := (len(sorted)*p + 99)/100 - 1
 if at < 0 { at = 0 }
 if at >= len(sorted) { at = len(sorted)-1 }
 return float64(sorted[at]) / 1000
}

func main() {
 endpoint:=flag.String("url","","Loopback HTTP endpoint")
 rate:=flag.Int("rate",500,"Offered requests/second")
 seconds:=flag.Int("seconds",15,"Steady load duration")
 output:=flag.String("output","","JSON file")
 flag.Parse()
 if *endpoint=="" || *output=="" || *rate<1 || *rate>10000 || *seconds<1 || *seconds>120 {
  fmt.Fprintln(os.Stderr,"invalid url/rate/seconds/output"); os.Exit(2)
 }
 var dials atomic.Uint64
 dialer:=net.Dialer{Timeout:2*time.Second,KeepAlive:30*time.Second}
 transport:=&http.Transport{
  MaxIdleConns:64, MaxIdleConnsPerHost:32, MaxConnsPerHost:32,
  IdleConnTimeout:30*time.Second, DisableCompression:true,
  DialContext:func(ctx context.Context,network,addr string)(net.Conn,error){
   dials.Add(1)
   return dialer.DialContext(ctx,network,addr)
  },
 }
 defer transport.CloseIdleConnections()
 client:=&http.Client{Transport:transport,Timeout:3*time.Second}
 total:=*rate * *seconds
 samples:=make(chan Sample,total)
 sem:=make(chan struct{},128)
 var wg sync.WaitGroup
 begin:=time.Now().Add(150*time.Millisecond)
 dropped,launched:=0,0
 for i:=0;i<total;i++ {
  planned:=begin.Add(time.Duration(i)*time.Second/time.Duration(*rate))
  if wait:=time.Until(planned);wait>0 { time.Sleep(wait) }
  if time.Since(planned)>500*time.Millisecond {dropped++;continue}
  select {
  case sem<-struct{}{}:
   launched++
   wg.Add(1)
   go func(planned time.Time){
    defer wg.Done()
    defer func(){<-sem}()
    ctx,cancel:=context.WithTimeout(context.Background(),3*time.Second)
    defer cancel()
    req,err:=http.NewRequestWithContext(ctx,http.MethodGet,*endpoint,nil)
    if err!=nil {samples<-Sample{Error:true};return}
    resp,err:=client.Do(req)
    if err!=nil {samples<-Sample{Error:true};return}
    _,err=io.Copy(io.Discard,resp.Body)
    closeErr:=resp.Body.Close()
    if err!=nil||closeErr!=nil {samples<-Sample{Status:resp.StatusCode,Error:true};return}
    samples<-Sample{Status:resp.StatusCode,LatencyMicros:time.Since(planned).Microseconds()}
   }(planned)
  default:dropped++
  }
 }
 wg.Wait()
 elapsed:=time.Since(begin).Seconds()
 close(samples)
 latencies:=make([]int64,0,total)
 completed,errors,non200,successful:=0,0,0,0
 for s:=range samples {
  completed++
  if s.Error { errors++ } else if s.Status!=200 { non200++ } else {
   successful++;latencies=append(latencies,s.LatencyMicros)
  }
 }
 sort.Slice(latencies,func(i,j int)bool{return latencies[i]<latencies[j]})
 report:=map[string]any{
  "target_rps":*rate, "duration_seconds":*seconds, "planned":total,
  "launched":launched, "dropped":dropped, "completed":completed,
  "successful":successful, "errors":errors, "http_non200":non200,
  "new_client_connections":dials.Load(),
  "p50_ms":percentile(latencies,50), "p95_ms":percentile(latencies,95),
  "p99_ms":percentile(latencies,99),
  "wall_seconds":elapsed, "goodput_rps":float64(successful)/float64(*seconds),
 }
 data,err:=json.MarshalIndent(report,"","  ");if err!=nil{panic(err)}
 if err=os.WriteFile(*output,append(data,'\n'),0644);err!=nil{panic(err)}
 fmt.Printf("FIXED_RATE_METRIC rate=%d goodput=%d dropped=%d errors=%d non200=%d p95_ms=%.3f client_tcp_new=%d\n",
  *rate,successful / *seconds,dropped,errors,non200,percentile(latencies,95),dials.Load())
 if dropped>0||errors>0||non200>0 {
  fmt.Fprintln(os.Stderr,"FIXED_RATE_UNHEALTHY: metrics preserved; do not treat this level as valid equal load")
 }
}
