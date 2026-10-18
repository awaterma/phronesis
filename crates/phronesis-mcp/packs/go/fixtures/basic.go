// Package main is a tiny fixture: a free function, a pointer-receiver
// method, grouped imports (plain, aliased, blank, dot), and both an
// unqualified and a selector call. None of the text below the imports is a
// declaration or a call:
//
//   func lookalike() {}
//   fmt.Println("also not a call")
//
package main

import (
	"fmt"
	str "strings"
	_ "unsafe"
	. "errors"
)

func greet(name string) string {
	msg := fmt.Sprintf("hello %s", name)
	return msg
}

type Greeter struct {
	Prefix string
}

func (g *Greeter) Greet(name string) string {
	return greet(name)
}

func useStrings() string {
	return str.ToUpper("hi")
}
